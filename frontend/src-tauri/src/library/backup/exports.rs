//! Portable Microsoft export history, scoped to the meetings in the archive.

use crate::exports::ledger::{meeting_dir, ExportLedger, LEDGER_FILENAME, MAX_LEDGER_BYTES};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use super::{failure, text_field, Manifest, MAX_MANIFEST};

pub(super) fn capture(manifest: &mut Manifest, app_data: &Path) -> Result<(), String> {
    let mut budget = MAX_MANIFEST;
    for (index, row) in manifest.tables["meetings"].iter().enumerate() {
        let id = text_field(row, "id")?;
        if let Some(mut ledger) = ExportLedger::read(&meeting_dir(app_data, id))? {
            ledger.retain_microsoft_links();
            ledger.validate_archive(id)?;
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

#[derive(Default)]
pub(super) struct PublishedHistory(Vec<PathBuf>);

impl PublishedHistory {
    pub(super) fn keep(mut self) {
        self.0.clear();
    }
}

impl Drop for PublishedHistory {
    fn drop(&mut self) {
        for path in self.0.drain(..) {
            let _ = std::fs::remove_file(path);
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
        if ExportLedger::read(&dir)?.as_ref() == Some(&ledger) {
            // A previous restore may have published the file before interruption.
            continue;
        }
        ledger.restore(&dir)?;
        published.0.push(dir.join(LEDGER_FILENAME));
    }
    Ok(published)
}
