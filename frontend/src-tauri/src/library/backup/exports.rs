//! Portable Microsoft export history, scoped to the meetings in the archive.

use crate::exports::ledger::{meeting_dir, ExportLedger, LEDGER_FILENAME, MAX_LEDGER_BYTES};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use super::{failure, meeting_failure, text_field, Manifest, MAX_MANIFEST};

pub(super) const UNREADABLE_HISTORY: &str =
    "Microsoft export history could not be read. Check file access or recover the history from a known-good copy before retrying.";

pub(super) fn capture(manifest: &mut Manifest, app_data: &Path) -> Result<(), String> {
    let mut budget = MAX_MANIFEST;
    for (index, row) in manifest.tables["meetings"].iter().enumerate() {
        let id = text_field(row, "id")?;
        if let Some(mut ledger) = ExportLedger::read(&meeting_dir(app_data, id))
            .map_err(|_| meeting_failure(row, UNREADABLE_HISTORY))? {
            ledger.retain_microsoft_links();
            ledger.validate_archive(id).map_err(|_| meeting_failure(row, UNREADABLE_HISTORY))?;
            debit_budget(&mut budget, &ledger)?;
            manifest.export_ledgers.insert(index, ledger);
        }
    }
    validate(manifest)
}

pub(super) fn capture_legacy_folder(
    ledgers: &mut BTreeMap<usize, ExportLedger>,
    index: usize,
    meeting_id: &str,
    folder: &Path,
    budget: &mut u64,
) -> Result<(), String> {
    if ledgers.contains_key(&index) {
        return Ok(());
    }
    if let Some(mut ledger) = ExportLedger::read(folder)? {
        ledger.retain_microsoft_links();
        ledger.validate_archive(meeting_id)?;
        debit_budget(budget, &ledger)?;
        ledgers.insert(index, ledger);
    }
    Ok(())
}

pub(super) fn remaining_budget(ledgers: &BTreeMap<usize, ExportLedger>) -> Result<u64, String> {
    ledgers
        .values()
        .try_fold(MAX_MANIFEST, |mut budget, ledger| {
            debit_budget(&mut budget, ledger)?;
            Ok(budget)
        })
}

fn debit_budget(budget: &mut u64, ledger: &ExportLedger) -> Result<(), String> {
    let size = serde_json::to_vec(ledger).map_err(failure)?.len() as u64;
    *budget = budget
        .checked_sub(size)
        .ok_or("Export history exceeds the archive metadata limit.")?;
    Ok(())
}

pub(super) fn validate(manifest: &Manifest) -> Result<(), String> {
    let mut scopes = HashMap::new();
    for (index, ledger) in &manifest.export_ledgers {
        let row = manifest.tables["meetings"]
            .get(*index)
            .ok_or("Invalid archive export history reference.")?;
        let id = text_field(row, "id")?;
        let scope = meeting_dir(Path::new(""), id)
            .to_string_lossy()
            .to_ascii_lowercase();
        if let Some(other) = scopes.insert(scope, id) {
            if other != id {
                return Err("Archive contains conflicting export history scopes.".into());
            }
        }
        ledger.validate_archive(id)?;
    }
    Ok(())
}

pub(super) fn read_legacy<R: Read + Seek>(
    manifest: &mut Manifest,
    zip: &mut zip::ZipArchive<R>,
    mut budget: u64,
) -> Result<(), String> {
    let mut history_only = Vec::new();
    for (index, names) in &mut manifest.files {
        let Some(position) = names.iter().position(|name| name == LEDGER_FILENAME) else {
            continue;
        };
        names.remove(position);
        if names.is_empty() {
            history_only.push(*index);
        }
        if manifest.export_ledgers.contains_key(index) {
            return Err("Archive contains duplicate export history.".into());
        }
        let mut file = zip
            .by_name(&format!("recordings/{index}/{LEDGER_FILENAME}"))
            .map_err(failure)?;
        if file.size() > MAX_LEDGER_BYTES {
            return Err("Legacy export history exceeds the archive metadata limit.".into());
        }
        budget = budget
            .checked_sub(file.size())
            .ok_or("Legacy export history exceeds the archive metadata limit.")?;
        let mut body = Vec::new();
        file.by_ref()
            .take(MAX_LEDGER_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(failure)?;
        if body.len() as u64 != file.size() {
            return Err("Legacy export history is truncated.".into());
        }
        manifest
            .export_ledgers
            .insert(*index, serde_json::from_slice(&body).map_err(failure)?);
    }
    for index in history_only {
        manifest.files.remove(&index);
    }
    Ok(())
}

pub(super) fn selected(manifest: &Manifest, meetings: &HashSet<String>) -> Vec<ExportLedger> {
    manifest
        .export_ledgers
        .values()
        .filter(|ledger| meetings.contains(&ledger.meeting_id))
        .cloned()
        .collect()
}

/// Tracks ledger files written during a restore so they can be rolled back if
/// the database transaction fails.  Each entry is `(path, previous_bytes)`:
/// `None` means the file was newly created (delete on rollback); `Some` holds
/// the original content to restore atomically.
#[derive(Default)]
pub(super) struct PublishedHistory(Vec<(PathBuf, Option<Vec<u8>>)>);

impl PublishedHistory {
    pub(super) fn keep(mut self) {
        self.0.clear();
    }
}

impl Drop for PublishedHistory {
    fn drop(&mut self) {
        for (path, previous) in self.0.drain(..) {
            match previous {
                None => {
                    let _ = std::fs::remove_file(&path);
                }
                Some(bytes) => {
                    // Best-effort: restore previous content atomically.
                    if let Some(parent) = path.parent() {
                        if let Ok(mut tmp) = tempfile::NamedTempFile::new_in(parent) {
                            if tmp.write_all(&bytes).is_ok() {
                                let _ = tmp.persist(&path);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Read the raw bytes of a ledger file without parsing (bounded read).
fn read_ledger_bytes(folder: &Path) -> Result<Option<Vec<u8>>, String> {
    let path = folder.join(LEDGER_FILENAME);
    match std::fs::File::open(&path) {
        Ok(file) => {
            let mut body = Vec::new();
            file.take(MAX_LEDGER_BYTES + 1)
                .read_to_end(&mut body)
                .map_err(|_| "Could not read export history.".to_string())?;
            if body.len() as u64 > MAX_LEDGER_BYTES {
                return Err("Export history exceeds the supported size.".into());
            }
            Ok(Some(body))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => {
            Err("Could not read export history. Restore access before exporting.".into())
        }
    }
}

pub(super) fn publish(
    ledgers: Vec<ExportLedger>,
    app_data: Option<&Path>,
) -> Result<PublishedHistory, String> {
    let mut published = PublishedHistory::default();
    for mut ledger in ledgers {
        ledger.retain_microsoft_links();
        let root = app_data.ok_or("Microsoft export history requires an app data directory.")?;
        let dir = meeting_dir(root, &ledger.meeting_id);
        let path = dir.join(LEDGER_FILENAME);

        // Read existing file once (raw bytes + parsed) so we can compare, merge,
        // and roll back atomically without reading the file a second time.
        let prev_bytes = read_ledger_bytes(&dir)?;

        match prev_bytes {
            None => {
                // No existing file: create new, roll back by deleting on failure.
                ledger.restore(&dir)?;
                published.0.push((path, None));
            }
            Some(ref bytes) => {
                let mut existing: ExportLedger =
                    serde_json::from_slice(bytes).map_err(|_| {
                        "Export history is invalid. Restore it before continuing to avoid duplicates.".to_string()
                    })?;

                existing.retain_microsoft_links();
                existing.validate_archive(&ledger.meeting_id)?;
                if existing == ledger {
                    // A previous restore wrote this exact file; nothing to do.
                    continue;
                }

                // Merge incoming history into the existing file, then overwrite.
                // On rollback the previous bytes are restored atomically.
                let mut merged = existing;
                merged.merge_incoming(&ledger);
                merged.validate_archive(&ledger.meeting_id)?;
                merged.save(&dir)?;
                published.0.push((path, prev_bytes));
            }
        }
    }
    Ok(published)
}
