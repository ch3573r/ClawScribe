//! Portable Microsoft export history, scoped to the meetings in the archive.

use crate::exports::ledger::{meeting_dir, ExportLedger, LEDGER_FILENAME, MAX_LEDGER_BYTES};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use super::{
    backup_checkpoint, failure, meeting_failure, text_field, Manifest, ScheduledBackup,
    MAX_MANIFEST,
};

pub(super) const UNREADABLE_HISTORY: &str =
    "Microsoft export history could not be read. Check file access or recover the history from a known-good copy before retrying.";

#[cfg(test)]
pub(super) fn capture(manifest: &mut Manifest, app_data: &Path) -> Result<(), String> {
    capture_for_backup(manifest, app_data, None)
}

pub(super) fn capture_for_backup(
    manifest: &mut Manifest,
    app_data: &Path,
    scheduled: Option<&ScheduledBackup>,
) -> Result<(), String> {
    let mut budget = MAX_MANIFEST;
    for (index, row) in manifest.tables["meetings"].iter().enumerate() {
        backup_checkpoint(scheduled)?;
        let id = text_field(row, "id")?;
        if let Some(mut ledger) = ExportLedger::read(&meeting_dir(app_data, id))
            .map_err(|_| meeting_failure(row, UNREADABLE_HISTORY))?
        {
            ledger.retain_microsoft_links();
            ledger
                .validate_archive(id)
                .map_err(|_| meeting_failure(row, UNREADABLE_HISTORY))?;
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

pub(super) struct RestoreHistory {
    ledger: ExportLedger,
    meeting_title: String,
}

pub(super) fn selected(manifest: &Manifest, meetings: &HashSet<String>) -> Vec<RestoreHistory> {
    manifest
        .export_ledgers
        .iter()
        .filter(|(_, ledger)| meetings.contains(&ledger.meeting_id))
        .map(|(index, ledger)| {
            let title: String = manifest.tables["meetings"][*index]
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .trim()
                .chars()
                .filter(|c| !c.is_control())
                .take(160)
                .collect();
            RestoreHistory {
                ledger: ledger.clone(),
                meeting_title: if title.trim().is_empty() {
                    "Untitled meeting".into()
                } else {
                    title
                },
            }
        })
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
fn read_ledger_bytes(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::File::open(path) {
        Ok(file) => {
            let mut body = Vec::new();
            file.take(MAX_LEDGER_BYTES + 1).read_to_end(&mut body)?;
            if body.len() as u64 > MAX_LEDGER_BYTES {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            Ok(Some(body))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub(super) fn publish(
    ledgers: Vec<RestoreHistory>,
    app_data: Option<&Path>,
) -> Result<PublishedHistory, String> {
    let mut published = PublishedHistory::default();
    for RestoreHistory {
        mut ledger,
        meeting_title,
    } in ledgers
    {
        ledger.retain_microsoft_links();
        let root = app_data.ok_or("Microsoft export history requires an app data directory.")?;
        let dir = meeting_dir(root, &ledger.meeting_id);
        let path = dir.join(LEDGER_FILENAME);
        let local_history_error = || {
            let location = meeting_dir(Path::new(""), &ledger.meeting_id).join(LEDGER_FILENAME);
            format!(
                "Cannot restore meeting \"{meeting_title}\" because its local Microsoft export history could not be read or validated. \
                 Check access to '{}' in ClawScribe's app data or restore the file from a recent known-good copy for this meeting, then retry. \
                 Deleting the history removes duplicate protection. Check the Microsoft destination before resubmitting tasks or pages.",
                location.display()
            )
        };

        // Read existing file once (raw bytes + parsed) so we can compare, merge,
        // and roll back atomically without reading the file a second time.
        let prev_bytes = read_ledger_bytes(&path).map_err(|_| local_history_error())?;

        match prev_bytes {
            None => {
                // No existing file: create new, roll back by deleting on failure.
                ledger.restore(&dir)?;
                published.0.push((path, None));
            }
            Some(ref bytes) => {
                let mut existing: ExportLedger =
                    serde_json::from_slice(bytes).map_err(|_| local_history_error())?;

                existing.retain_microsoft_links();
                existing
                    .validate_archive(&ledger.meeting_id)
                    .map_err(|_| local_history_error())?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exports::ledger::task_key;

    #[test]
    fn damaged_local_history_names_archive_meeting_and_rolls_back_prior_merge() {
        let dir = tempfile::tempdir().unwrap();
        let first_id = "test-meeting-first";
        let damaged_id = "test-meeting-damaged";
        let first_dir = meeting_dir(dir.path(), first_id);
        ExportLedger::new(first_id).save(&first_dir).unwrap();
        let first_path = first_dir.join(LEDGER_FILENAME);
        let original = std::fs::read(&first_path).unwrap();
        let mut incoming = ExportLedger::new(first_id);
        let key = task_key(
            "todo:tenant:user:list",
            first_id,
            "action-1",
            "0123456789abcdef",
        );
        incoming.record_success(&key, Some("task-id".into()), None, None);
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "format": "test", "version": 2, "files": {},
            "tables": {"meetings": [
                {"id": first_id, "title": "First meeting"},
                {"id": damaged_id, "title": "Planning meeting"}
            ]},
            "export_ledgers": {"0": incoming, "1": ExportLedger::new(damaged_id)}
        }))
        .unwrap();
        let meeting_ids = HashSet::from([first_id.into(), damaged_id.into()]);
        let damaged_dir = meeting_dir(dir.path(), damaged_id);
        std::fs::create_dir_all(&damaged_dir).unwrap();
        let damaged_path = damaged_dir.join(LEDGER_FILENAME);

        let invalid_json = b"not valid json".to_vec();
        let wrong_meeting = serde_json::to_vec(&ExportLedger::new(first_id)).unwrap();
        for body in [Some(invalid_json), Some(wrong_meeting), None] {
            if let Some(bytes) = &body {
                std::fs::write(&damaged_path, bytes).unwrap();
            } else {
                std::fs::create_dir(&damaged_path).unwrap();
            }
            let ledgers = selected(&manifest, &meeting_ids);
            let error = publish(ledgers, Some(dir.path())).err().unwrap();
            assert!(error.contains("Planning meeting"), "{error}");
            assert!(error.contains("local Microsoft export history"), "{error}");
            assert!(error.contains(LEDGER_FILENAME), "{error}");
            assert!(error.contains("known-good copy"), "{error}");
            assert!(error.contains("duplicate protection"), "{error}");
            assert_eq!(std::fs::read(&first_path).unwrap(), original);
            if let Some(bytes) = body {
                assert_eq!(std::fs::read(&damaged_path).unwrap(), bytes);
                std::fs::remove_file(&damaged_path).unwrap();
            } else {
                assert!(damaged_path.is_dir());
            }
        }
    }
}
