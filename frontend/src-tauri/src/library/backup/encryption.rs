//! Versioned encryption envelope for the portable ZIP. Each frame authenticates
//! the header, its position, and whether it is the final frame.
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::stream::{DecryptorBE32, EncryptorBE32},
    XChaCha20Poly1305,
};
use rand::{rngs::OsRng, RngCore};
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::Path;
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
        if total > MAX_ZIP {
            return Err("Archive is too large.".into());
        }
        // Check the first tag before opening a temporary ZIP. Use an unnamed
        // local file rather than a plaintext copy next to the selected backup.
        let output = match &mut zip {
            Some(output) => output,
            None => zip.insert(scratch_file(scratch)?),
        };
        output.write_all(&buffer).map_err(super::failure)?;
        buffer.zeroize();
    }
    if total + buffer.len() as u64 > MAX_ZIP {
        return Err("Archive is too large.".into());
    }
    let mut zip = match zip {
        Some(zip) => zip,
        None => scratch_file(scratch)?,
    };
    zip.write_all(&buffer).map_err(super::failure)?;
    zip.rewind().map_err(super::failure)?;
    Ok(zip)
}

pub(super) fn scratch_file(scratch: &Path) -> Result<File, String> {
    std::fs::create_dir_all(scratch).map_err(super::failure)?;
    tempfile::tempfile_in(scratch).map_err(super::failure)
}
