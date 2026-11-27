//! Versioned encryption envelope for the portable ZIP. Each frame authenticates
//! the header, its position, and whether it is the final frame.
use crate::audio::disk_space::{free_space_volume, START_FLOOR_BYTES};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::stream::{DecryptorBE32, EncryptorBE32},
    XChaCha20Poly1305,
};
use rand::{rngs::OsRng, RngCore};
use std::fs::{self, File};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use zeroize::{Zeroize, Zeroizing};

pub(crate) const EXTENSION: &str = "clawbackup";
pub(super) const PASSPHRASE_REQUIRED: &str = "Enter the passphrase for this backup.";
const AUTHENTICATION_FAILED: &str =
    "The passphrase is incorrect or the encrypted backup is damaged.";
const MAGIC: &[u8; 8] = b"CLAWBAK\0";
const FORMAT: u8 = 1;
const SALT_START: usize = MAGIC.len() + 1;
const NONCE_START: usize = SALT_START + 16;
const HEADER_SIZE: usize = NONCE_START + 19;
const FRAME: usize = 1024 * 1024;
const TAG: usize = 16;
pub(super) const SCRATCH_FULL: &str =
    "Not enough free disk space for the temporary backup on the local drive. Free up space and try again.";
// The ZIP limit covers expanded entries separately. Allow room for its
// manifest, local/central headers and ZIP64 metadata outside that budget.
const MAX_ZIP: u64 = super::MAX_BYTES + super::MAX_MANIFEST + 128 * 1024 * 1024;

fn derive_key(passphrase: &str, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>, String> {
    validate_passphrase(passphrase)?;
    // Fixed version 1 parameters. An input file cannot select the work factor.
    let params = Params::new(64 * 1024, 3, 4, Some(32)).map_err(super::failure)?;
    let mut key = Zeroizing::new([0; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, key.as_mut())
        .map_err(super::failure)?;
    Ok(key)
}

pub(super) fn validate_passphrase(passphrase: &str) -> Result<(), String> {
    if passphrase.trim().is_empty() || passphrase.len() > 1024 {
        return Err("Enter a passphrase of up to 1,024 bytes.".into());
    }
    Ok(())
}

pub(super) fn seal(
    mut zip: File,
    output: &mut File,
    passphrase: Zeroizing<String>,
) -> Result<(), String> {
    if zip.metadata().map_err(super::failure)?.len() > MAX_ZIP {
        return Err("Archive is too large.".into());
    }
    zip.rewind().map_err(super::failure)?;
    let mut header = [0; HEADER_SIZE];
    header[..MAGIC.len()].copy_from_slice(MAGIC);
    header[MAGIC.len()] = FORMAT;
    OsRng
        .try_fill_bytes(&mut header[SALT_START..])
        .map_err(super::failure)?;
    let key = derive_key(&passphrase, &header[SALT_START..NONCE_START])?;
    drop(passphrase);
    let mut encrypt =
        EncryptorBE32::<XChaCha20Poly1305>::new((&*key).into(), (&header[NONCE_START..]).into());
    drop(key);
    output.write_all(&header).map_err(super::failure)?;
    let mut buffer = Zeroizing::new(Vec::with_capacity(FRAME + TAG));
    loop {
        (&mut zip)
            .take(FRAME as u64)
            .read_to_end(&mut buffer)
            .map_err(super::failure)?;
        if buffer.len() < FRAME {
            encrypt
                .encrypt_last_in_place(&header, &mut *buffer)
                .map_err(super::failure)?;
            return output.write_all(&buffer).map_err(super::failure);
        }
        encrypt
            .encrypt_next_in_place(&header, &mut *buffer)
            .map_err(super::failure)?;
        output.write_all(&buffer).map_err(super::failure)?;
        buffer.zeroize();
    }
}

pub(super) fn open_zip(
    path: &Path,
    scratch: &Path,
    passphrase: Option<Zeroizing<String>>,
) -> Result<File, String> {
    let mut input = File::open(path).map_err(super::failure)?;
    let mut header = [0; HEADER_SIZE];
    let mut magic = Vec::with_capacity(MAGIC.len());
    (&mut input)
        .take(MAGIC.len() as u64)
        .read_to_end(&mut magic)
        .map_err(super::failure)?;
    if magic != MAGIC {
        if passphrase.is_some()
            || path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case(EXTENSION))
        {
            return Err("The encrypted backup header is invalid.".into());
        }
        input.rewind().map_err(super::failure)?;
        return Ok(input);
    }
    header[..MAGIC.len()].copy_from_slice(MAGIC);
    input
        .read_exact(&mut header[MAGIC.len()..])
        .map_err(super::failure)?;
    if header[MAGIC.len()] != FORMAT {
        return Err("This encrypted backup format is unsupported.".into());
    }
    let passphrase = passphrase.ok_or(PASSPHRASE_REQUIRED)?;
    let scratch = prepare_scratch(scratch)?;
    let encrypted_bytes = input.metadata().map_err(super::failure)?.len();
    let payload = encrypted_bytes.saturating_sub(HEADER_SIZE as u64);
    let frames = payload.div_ceil((FRAME + TAG) as u64);
    let estimate = payload
        .checked_sub(frames * TAG as u64)
        .ok_or(AUTHENTICATION_FAILED)?;
    if estimate > MAX_ZIP {
        return Err("Archive is too large.".into());
    }
    let key = derive_key(&passphrase, &header[SALT_START..NONCE_START])?;
    drop(passphrase);
    let mut decrypt =
        DecryptorBE32::<XChaCha20Poly1305>::new((&*key).into(), (&header[NONCE_START..]).into());
    drop(key);
    let mut buffer = Zeroizing::new(Vec::with_capacity(FRAME + TAG));
    let mut zip = None;
    let mut total = 0;
    loop {
        (&mut input)
            .take((FRAME + TAG) as u64)
            .read_to_end(&mut buffer)
            .map_err(super::failure)?;
        if buffer.len() < FRAME + TAG {
            decrypt
                .decrypt_last_in_place(&header, &mut *buffer)
                .map_err(|_| AUTHENTICATION_FAILED)?;
            break;
        }
        decrypt
            .decrypt_next_in_place(&header, &mut *buffer)
            .map_err(|_| AUTHENTICATION_FAILED)?;
        total += buffer.len() as u64;
        if total > estimate {
            return Err(AUTHENTICATION_FAILED.into());
        }
        // Check the first tag before opening a temporary ZIP. Use an unnamed
        // local file rather than a plaintext copy next to the selected backup.
        let output = match &mut zip {
            Some(output) => output,
            None => {
                check_space(free_space_volume(&scratch).map(|(_, free)| free), estimate)?;
                zip.insert(scratch_file(&scratch)?)
            }
        };
        output.write_all(&buffer).map_err(super::failure)?;
        buffer.zeroize();
    }
    if total + buffer.len() as u64 != estimate {
        return Err(AUTHENTICATION_FAILED.into());
    }
    let mut zip = match zip {
        Some(zip) => zip,
        None => {
            check_space(free_space_volume(&scratch).map(|(_, free)| free), estimate)?;
            scratch_file(&scratch)?
        }
    };
    zip.write_all(&buffer).map_err(super::failure)?;
    zip.rewind().map_err(super::failure)?;
    Ok(zip)
}

pub(super) fn scratch_file(scratch: &Path) -> Result<File, String> {
    tempfile::tempfile_in(scratch).map_err(super::failure)
}

fn check_space(free: Option<u64>, needed: u64) -> Result<u64, String> {
    let free = free.ok_or("Could not verify free disk space for the temporary backup.")?;
    let budget = free.checked_sub(START_FLOOR_BYTES).ok_or(SCRATCH_FULL)?;
    if needed > budget {
        return Err(SCRATCH_FULL.into());
    }
    Ok(budget)
}

pub(super) fn backup_scratch_limit(
    scratch: &Path,
    destination: &Path,
    estimate: u64,
) -> Result<u64, String> {
    let space = free_space_volume(scratch);
    let same_volume = space
        .as_ref()
        .zip(destination.parent().and_then(free_space_volume))
        .is_some_and(|((scratch_volume, _), (destination_volume, _))| {
            scratch_volume == &destination_volume
        });
    let budget = check_space(space.map(|(_, free)| free), estimate)?;
    if same_volume {
        // The plaintext and ciphertext coexist until the completed backup is published.
        let tag_budget = (budget / 2 / FRAME as u64 + 1) * TAG as u64 + HEADER_SIZE as u64;
        let limit = budget.saturating_sub(tag_budget) / 2;
        if estimate > limit {
            return Err(SCRATCH_FULL.into());
        }
        return Ok(limit);
    }
    Ok(budget)
}

pub(super) fn prepare_scratch(local_data: &Path) -> Result<PathBuf, String> {
    let scratch = local_data.join("backup-scratch");
    fs::create_dir_all(&scratch).map_err(super::failure)?;
    let _pins =
        crate::library::archive::storage::pin_backup_folder(&scratch).map_err(super::failure)?;
    for entry in fs::read_dir(&scratch).map_err(super::failure)? {
        let entry = entry.map_err(super::failure)?;
        fs::remove_file(entry.path()).map_err(super::failure)?;
    }
    Ok(scratch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_sweep_is_scoped_and_reserves_free_space() {
        let root = tempfile::tempdir().unwrap();
        let scratch = prepare_scratch(root.path()).unwrap();
        fs::write(scratch.join("abandoned.tmp"), b"synthetic archive").unwrap();
        fs::write(root.path().join("keep.txt"), b"keep").unwrap();
        assert_eq!(prepare_scratch(root.path()).unwrap(), scratch);
        assert_eq!(fs::read_dir(scratch).unwrap().count(), 0);
        assert!(root.path().join("keep.txt").exists());
        assert_eq!(check_space(Some(START_FLOOR_BYTES + 8), 8).unwrap(), 8);
        assert_eq!(
            check_space(Some(START_FLOOR_BYTES + 8), 9).unwrap_err(),
            SCRATCH_FULL
        );
    }
}
