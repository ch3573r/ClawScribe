//! `exports.json` idempotency ledger.
//!
//! Lives in a recording folder and records, per dedupe key, what has already
//! been exported so a succeeded page/task is never created twice. It contains
//! only destination IDs, dedupe keys, Graph resource IDs, URLs, timestamps,
//! statuses, and sanitized error codes — never tokens. See the idempotency
//! sections of the OneNote/Planner design docs.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::exports::model::ExportStatus;

pub const LEDGER_SCHEMA: &str = "clawscribe.exports.v1";
pub const LEDGER_FILENAME: &str = "exports.json";

/// One export record, keyed by its dedupe key in [`ExportLedger::entries`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEntry {
    pub status: ExportStatus,
    /// OneNote `pageId` or Planner `taskId` once created.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub resource_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub web_url: Option<String>,
    /// Sanitized Graph error code (never a body or token).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub code: Option<String>,
    pub attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_attempt_at: Option<String>,
}

impl LedgerEntry {
    fn new_pending(now: Option<String>) -> Self {
        LedgerEntry {
            status: ExportStatus::Pending,
            resource_id: None,
            web_url: None,
            code: None,
            attempts: 1,
            last_attempt_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportLedger {
    pub schema: String,
    pub meeting_id: String,
    /// Keyed by dedupe key. Covers both OneNote pages and Planner tasks.
    #[serde(default)]
    pub entries: BTreeMap<String, LedgerEntry>,
    #[serde(skip)]
    storage_dir: Option<PathBuf>,
}

impl ExportLedger {
    pub fn new(meeting_id: impl Into<String>) -> Self {
        ExportLedger {
            schema: LEDGER_SCHEMA.to_string(),
            meeting_id: meeting_id.into(),
            entries: BTreeMap::new(),
            storage_dir: None,
        }
    }

    /// Load the ledger for a recording folder, or start a fresh one if absent.
    pub fn load_or_new(folder: &Path, meeting_id: &str) -> Result<Self, String> {
        let mut ledger = match std::fs::read_to_string(folder.join(LEDGER_FILENAME)) {
            Ok(raw) => serde_json::from_str::<Self>(&raw).map_err(|_| {
                "Export history is invalid. Restore it before exporting to avoid duplicates."
            })?,
            Err(error) if error.kind() == ErrorKind::NotFound => Self::new(meeting_id),
            Err(_) => {
                return Err(
                    "Could not read export history. Restore access before exporting.".into(),
                )
            }
        };
        if ledger.schema != LEDGER_SCHEMA || ledger.meeting_id != meeting_id {
            return Err("Export history does not match this meeting or supported format.".into());
        }
        // A crash after submission can leave a pending create. Never replay it.
        for entry in ledger.entries.values_mut() {
            if entry.status == ExportStatus::Pending {
                entry.status = ExportStatus::UnknownAfterSubmit;
                entry.code = Some("interrupted_export_review_required".into());
            }
        }
        ledger.storage_dir = Some(folder.to_path_buf());
        Ok(ledger)
    }

    /// Atomically replace the history only after its new contents reach disk.
    pub fn save(&self, folder: &Path) -> Result<(), String> {
        std::fs::create_dir_all(folder).map_err(|_| "Could not create export history storage.")?;
        let mut file = tempfile::NamedTempFile::new_in(folder)
            .map_err(|_| "Could not stage export history.")?;
        let body = serde_json::to_vec(self).map_err(|_| "Could not serialize export history.")?;
        file.write_all(&body)
            .map_err(|_| "Could not write export history.")?;
        file.as_file()
            .sync_all()
            .map_err(|_| "Could not flush export history.")?;
        file.persist(folder.join(LEDGER_FILENAME))
            .map_err(|_| "Could not replace export history.")?;
        Ok(())
    }

    /// Persist before and after each remote create without blocking async work.
    pub async fn checkpoint(&self) -> Result<(), String> {
        let Some(folder) = self.storage_dir.clone() else {
            return Ok(());
        };
        let snapshot = self.clone();
        tokio::task::spawn_blocking(move || snapshot.save(&folder))
            .await
            .map_err(|_| "Export history writer failed.")?
    }

    pub fn entry(&self, dedupe_key: &str) -> Option<&LedgerEntry> {
        self.entries.get(dedupe_key)
    }

    /// Whether a Graph call should be skipped because this key already
    /// succeeded. Returns the existing entry so the UI can show its URL/ID.
    pub fn already_succeeded(&self, dedupe_key: &str) -> Option<&LedgerEntry> {
        self.entry(dedupe_key)
            .filter(|e| e.status.is_terminal_success())
    }

    /// Whether a new attempt is allowed for this key.
    ///
    /// - No entry, or a retriable failure: allowed.
    /// - Already succeeded: not allowed (use [`already_succeeded`]).
    /// - `UnknownAfterSubmit` / `DestinationNotFound`: not allowed without
    ///   explicit user review.
    pub fn may_attempt(&self, dedupe_key: &str) -> bool {
        match self.entry(dedupe_key) {
            None => true,
            Some(e) if e.status.is_terminal_success() => false,
            Some(e) => e.status.allows_automatic_retry(),
        }
    }

    /// Record the start of an attempt, incrementing the attempt counter.
    pub fn begin_attempt(&mut self, dedupe_key: &str, now: Option<String>) {
        self.entries
            .entry(dedupe_key.to_string())
            .and_modify(|e| {
                e.status = ExportStatus::Pending;
                e.attempts += 1;
                e.last_attempt_at = now.clone();
            })
            .or_insert_with(|| LedgerEntry::new_pending(now));
    }

    pub fn record_success(
        &mut self,
        dedupe_key: &str,
        resource_id: Option<String>,
        web_url: Option<String>,
        now: Option<String>,
    ) {
        let entry = self
            .entries
            .entry(dedupe_key.to_string())
            .or_insert_with(|| LedgerEntry::new_pending(now.clone()));
        entry.status = ExportStatus::Succeeded;
        entry.resource_id = resource_id;
        entry.web_url = web_url;
        entry.code = None;
        entry.last_attempt_at = now;
    }

    pub fn record_failure(
        &mut self,
        dedupe_key: &str,
        status: ExportStatus,
        code: Option<String>,
        now: Option<String>,
    ) {
        let entry = self
            .entries
            .entry(dedupe_key.to_string())
            .or_insert_with(|| LedgerEntry::new_pending(now.clone()));
        entry.status = status;
        entry.code = code;
        entry.last_attempt_at = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_and_omits_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let mut ledger = ExportLedger::new("m1");
        ledger.begin_attempt("onenote:k1", Some("t0".into()));
        ledger.record_success(
            "onenote:k1",
            Some("page-id".into()),
            Some("https://example/page".into()),
            Some("t1".into()),
        );
        ledger.save(dir.path()).unwrap();

        let raw = std::fs::read_to_string(dir.path().join(LEDGER_FILENAME)).unwrap();
        assert!(!raw.to_lowercase().contains("token"));
        assert!(!raw.to_lowercase().contains("bearer"));

        let loaded = ExportLedger::load_or_new(dir.path(), "m1").unwrap();
        assert_eq!(loaded.schema, LEDGER_SCHEMA);
        let entry = loaded.entry("onenote:k1").unwrap();
        assert_eq!(entry.status, ExportStatus::Succeeded);
        assert_eq!(entry.resource_id.as_deref(), Some("page-id"));
        assert_eq!(entry.attempts, 1);
    }

    #[test]
    fn load_missing_returns_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = ExportLedger::load_or_new(dir.path(), "m2").unwrap();
        assert_eq!(ledger.meeting_id, "m2");
        assert!(ledger.entries.is_empty());
    }

    #[test]
    fn atomic_save_leaves_no_tmp_and_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let mut ledger = ExportLedger::new("m1");
        ledger.record_failure("k", ExportStatus::Failed, Some("throttled".into()), None);
        ledger.save(dir.path()).unwrap();
        ledger.record_success("k", Some("id".into()), None, None);
        ledger.save(dir.path()).unwrap();

        assert!(!dir.path().join(format!("{LEDGER_FILENAME}.tmp")).exists());
        let loaded = ExportLedger::load_or_new(dir.path(), "m1").unwrap();
        assert_eq!(loaded.entry("k").unwrap().status, ExportStatus::Succeeded);
    }

    #[test]
    fn attempt_gating() {
        let mut ledger = ExportLedger::new("m1");
        assert!(ledger.may_attempt("new"));

        ledger.record_success("done", None, None, None);
        assert!(!ledger.may_attempt("done"));
        assert!(ledger.already_succeeded("done").is_some());

        ledger.record_failure("retriable", ExportStatus::Failed, None, None);
        assert!(ledger.may_attempt("retriable"));

        ledger.record_failure("review", ExportStatus::UnknownAfterSubmit, None, None);
        assert!(!ledger.may_attempt("review"));

        ledger.record_failure("gone", ExportStatus::DestinationNotFound, None, None);
        assert!(!ledger.may_attempt("gone"));
    }

    #[test]
    fn unreadable_or_mismatched_history_never_becomes_a_fresh_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LEDGER_FILENAME);
        std::fs::write(&path, "invalid history").unwrap();
        assert!(ExportLedger::load_or_new(dir.path(), "m1").is_err());
        let mut ledger = ExportLedger::new("another-meeting");
        ledger.save(dir.path()).unwrap();
        assert!(ExportLedger::load_or_new(dir.path(), "m1").is_err());
        ledger.meeting_id = "m1".into();
        ledger.schema = "unknown-schema".into();
        ledger.save(dir.path()).unwrap();
        assert!(ExportLedger::load_or_new(dir.path(), "m1").is_err());
    }

    #[tokio::test]
    async fn interrupted_creates_require_review_and_completed_creates_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut ledger = ExportLedger::load_or_new(dir.path(), "m1").unwrap();
        ledger.begin_attempt("pending", None);
        ledger.checkpoint().await.unwrap();
        assert!(!ledger.may_attempt("pending"));
        let interrupted = ExportLedger::load_or_new(dir.path(), "m1").unwrap();
        assert_eq!(
            interrupted.entry("pending").unwrap().status,
            ExportStatus::UnknownAfterSubmit
        );
        assert!(!interrupted.may_attempt("pending"));
        ledger.record_success("pending", Some("task-id".into()), None, None);
        ledger.checkpoint().await.unwrap();
        let completed = ExportLedger::load_or_new(dir.path(), "m1").unwrap();
        assert_eq!(
            completed
                .already_succeeded("pending")
                .unwrap()
                .resource_id
                .as_deref(),
            Some("task-id")
        );
        assert!(!std::fs::read_to_string(dir.path().join(LEDGER_FILENAME))
            .unwrap()
            .contains("storageDir"));
    }

    #[test]
    fn begin_attempt_increments() {
        let mut ledger = ExportLedger::new("m1");
        ledger.begin_attempt("k", Some("t0".into()));
        assert_eq!(ledger.entry("k").unwrap().attempts, 1);
        ledger.begin_attempt("k", Some("t1".into()));
        assert_eq!(ledger.entry("k").unwrap().attempts, 2);
    }
}
