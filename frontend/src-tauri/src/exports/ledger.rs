//! `exports.json` idempotency ledger.
//!
//! Lives in app data and records, per dedupe key, what has already
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
pub(crate) const MAX_LEDGER_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) fn meeting_dir(app_data: &Path, meeting_id: &str) -> PathBuf {
    let safe: String = meeting_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    app_data
        .join("export-ledgers")
        .join(if safe.trim_matches('-').is_empty() {
            "meeting"
        } else {
            &safe
        })
}

fn meeting_key_hash(meeting_id: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(meeting_id.as_bytes())[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn task_key(scope: &str, meeting_id: &str, action_id: &str, title_hash: &str) -> String {
    let meeting_hash = meeting_key_hash(meeting_id);
    if uuid::Uuid::parse_str(action_id).is_ok() {
        format!("{scope}:v2:{meeting_hash}:{action_id}")
    } else {
        // Old callers send positional IDs that can be reused for other titles.
        format!("{scope}:v2:{meeting_hash}:{action_id}:{title_hash}")
    }
}

/// One export record, keyed by its dedupe key in [`ExportLedger::entries`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OneNoteOptions {
    pub content: OneNoteContent,
    pub speakers: bool,
    pub timestamps: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OneNoteContent {
    Summary,
    Transcript,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OneNoteSection {
    pub tenant_id: String,
    pub user_id: String,
    pub notebook_id: String,
    pub section_id: Option<String>,
    pub section_name: String,
    pub page_title: String,
    pub content_hash: String,
    pub options: Option<OneNoteOptions>,
    pub started_at: String,
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExportLedger {
    pub schema: String,
    pub meeting_id: String,
    /// Keyed by dedupe key. Covers both OneNote pages and Planner tasks.
    #[serde(default)]
    pub entries: BTreeMap<String, LedgerEntry>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub onenote_sections: BTreeMap<String, OneNoteSection>,
    #[serde(skip)]
    storage_dir: Option<PathBuf>,
}

impl ExportLedger {
    pub fn new(meeting_id: impl Into<String>) -> Self {
        ExportLedger {
            schema: LEDGER_SCHEMA.to_string(),
            meeting_id: meeting_id.into(),
            entries: BTreeMap::new(),
            onenote_sections: BTreeMap::new(),
            storage_dir: None,
        }
    }

    /// Load the ledger for a meeting, or start a fresh one if absent.
    pub fn load_or_new(folder: &Path, meeting_id: &str) -> Result<Self, String> {
        let mut ledger = Self::read(folder)?.unwrap_or_else(|| Self::new(meeting_id));
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

    pub(crate) fn read(folder: &Path) -> Result<Option<Self>, String> {
        use std::io::Read;
        let file = match std::fs::File::open(folder.join(LEDGER_FILENAME)) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(_) => {
                return Err(
                    "Could not read export history. Restore access before exporting.".into(),
                )
            }
        };
        let mut body = Vec::new();
        file.take(MAX_LEDGER_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|_| "Could not read export history.")?;
        if body.len() as u64 > MAX_LEDGER_BYTES {
            return Err("Export history exceeds the supported size.".into());
        }
        serde_json::from_slice(&body).map(Some).map_err(|_| {
            "Export history is invalid. Restore it before exporting to avoid duplicates.".into()
        })
    }

    /// Atomically replace the history only after its new contents reach disk.
    pub fn save(&self, folder: &Path) -> Result<(), String> {
        self.write(folder, false)
    }

    pub(crate) fn restore(&self, folder: &Path) -> Result<(), String> {
        self.write(folder, true)
    }

    pub(crate) fn retain_microsoft_links(&mut self) {
        for entry in self.entries.values_mut() {
            entry.web_url = entry.web_url.as_deref().and_then(microsoft_web_url);
        }
    }

    pub(crate) fn validate_archive(&self, meeting_id: &str) -> Result<(), String> {
        let date = |value: &str| chrono::DateTime::parse_from_rfc3339(value).is_ok();
        let hash = |value: &str, length: usize| {
            value.len() == length && value.bytes().all(|b| b.is_ascii_hexdigit())
        };
        let text = |value: &str, length: usize| {
            !value.is_empty() && value.len() <= length && !value.chars().any(char::is_control)
        };
        let expected_meeting = meeting_key_hash(meeting_id);
        let valid_key = |key: &str| {
            if !text(key, 4096) {
                return false;
            }
            let fields: Vec<_> = key.split(':').collect();
            match fields.as_slice() {
                ["onenote", tenant, user, section, artifact, kind, index] => {
                    [tenant, user, section, kind]
                        .into_iter()
                        .all(|value| !value.is_empty())
                        && hash(artifact, 16)
                        && index.parse::<usize>().is_ok()
                }
                ["planner", _, _, _, _, tail @ ..] | ["todo", _, _, _, tail @ ..] => match tail {
                    ["v2", meeting_hash, action_id] => {
                        *meeting_hash == expected_meeting
                            && uuid::Uuid::parse_str(action_id).is_ok()
                    }
                    ["v2", meeting_hash, action_id, title_hash] => {
                        *meeting_hash == expected_meeting
                            && !action_id.is_empty()
                            && hash(title_hash, 16)
                    }
                    [artifact, action_id, title_hash] => {
                        hash(artifact, 16) && !action_id.is_empty() && hash(title_hash, 16)
                    }
                    _ => false,
                },
                _ => false,
            }
        };
        let invalid = self.schema != LEDGER_SCHEMA
            || self.meeting_id != meeting_id
            || self.entries.iter().any(|(key, entry)| {
                !valid_key(key)
                    || entry.attempts == 0
                    || entry
                        .resource_id
                        .as_deref()
                        .is_some_and(|id| !text(id, 2048))
                    || entry.code.as_deref().is_some_and(|code| !text(code, 128))
                    || entry
                        .last_attempt_at
                        .as_deref()
                        .is_some_and(|time| !date(time))
                    || entry
                        .web_url
                        .as_deref()
                        .is_some_and(|url| !text(url, 4096) || web_url(url).is_none())
            })
            || self.onenote_sections.iter().any(|(id, section)| {
                uuid::Uuid::parse_str(id).is_err()
                    || !valid_remote_id(&section.tenant_id)
                    || !valid_remote_id(&section.user_id)
                    || !valid_remote_id(&section.notebook_id)
                    || section
                        .section_id
                        .as_deref()
                        .is_some_and(|id| !valid_remote_id(id))
                    || !text(&section.section_name, 256)
                    || !text(&section.page_title, 4096)
                    || !hash(&section.content_hash, 64)
                    || !date(&section.started_at)
            });
        if invalid
            || serde_json::to_vec(self).map_or(true, |body| body.len() as u64 > MAX_LEDGER_BYTES)
        {
            return Err(
                "Archive contains invalid or out-of-scope Microsoft export history.".into(),
            );
        }
        Ok(())
    }

    fn write(&self, folder: &Path, create_only: bool) -> Result<(), String> {
        std::fs::create_dir_all(folder).map_err(|_| "Could not create export history storage.")?;
        let mut file = tempfile::NamedTempFile::new_in(folder)
            .map_err(|_| "Could not stage export history.")?;
        let body = serde_json::to_vec(self).map_err(|_| "Could not serialize export history.")?;
        if body.len() as u64 > MAX_LEDGER_BYTES {
            return Err("Export history exceeds the supported size.".into());
        }
        file.write_all(&body)
            .map_err(|_| "Could not write export history.")?;
        file.as_file()
            .sync_all()
            .map_err(|_| "Could not flush export history.")?;
        if create_only {
            file.persist_noclobber(folder.join(LEDGER_FILENAME))
                .map_err(|_| "Existing export history was kept. Restore into a fresh library or move the existing archive.")?;
        } else {
            file.persist(folder.join(LEDGER_FILENAME))
                .map_err(|_| "Could not replace export history.")?;
        }
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

    /// Resolve old task keys without relying on the hash of the original batch.
    /// Keep the original record so old callers and restored backups share it.
    pub(crate) fn existing_task_key(
        &self,
        scope: &str,
        meeting_id: &str,
        action_id: &str,
        title_hash: &str,
        legacy_action: Option<&super::actions::LegacyAction>,
    ) -> Option<String> {
        let current = task_key(scope, meeting_id, action_id, title_hash);
        let (old_id, old_hash) = legacy_action.map_or((action_id, title_hash), |legacy| {
            (&legacy.id, &legacy.title_hash)
        });
        let previous = [old_hash, title_hash].map(|hash| task_key(scope, meeting_id, old_id, hash));
        let suffixes = [old_hash, title_hash].map(|hash| format!(":{old_id}:{hash}"));
        self.entries
            .iter()
            .filter(|(key, _)| {
                if key.as_str() == current || previous.iter().any(|previous| previous == *key) {
                    return true;
                }
                key.strip_prefix(scope)
                    .and_then(|key| key.strip_prefix(':'))
                    .is_some_and(|key| {
                        suffixes.iter().any(|suffix| {
                            key.strip_suffix(suffix.as_str()).is_some_and(|batch| {
                                batch.len() == 16 && batch.bytes().all(|b| b.is_ascii_hexdigit())
                            })
                        })
                    })
            })
            .max_by_key(|(_, entry)| {
                (
                    entry.status.is_terminal_success(),
                    !entry.status.allows_automatic_retry(),
                    &entry.last_attempt_at,
                )
            })
            .map(|(key, _)| key.clone())
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

pub(crate) fn valid_remote_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 512
        && !matches!(id, "." | "..")
        && id.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'!' | b'=' | b'+')
        })
}

fn web_url(value: &str) -> Option<url::Url> {
    let url = url::Url::parse(value).ok()?;
    (matches!(url.scheme(), "https" | "http")
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}

pub(crate) fn microsoft_web_url(value: &str) -> Option<String> {
    let url = web_url(value)?;
    let host = url.host_str()?;
    let fragment_pairs = url.fragment().into_iter().flat_map(|fragment| {
        url::form_urlencoded::parse(fragment.trim_start_matches('?').as_bytes())
    });
    if url.query_pairs().chain(fragment_pairs).any(|(name, _)| {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "access_token" | "refresh_token" | "id_token" | "authkey" | "token" | "code" | "sig"
        )
    }) {
        return None;
    }
    (url.scheme() == "https"
        && [
            "onenote.com",
            "office.com",
            "office365.com",
            "microsoft.com",
            "sharepoint.com",
            "live.com",
            "microsoft365.com",
            "cloud.microsoft",
        ]
        .into_iter()
        .any(|domain| {
            host == domain
                || host
                    .strip_suffix(domain)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        }))
    .then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_links_drop_auth_parameters_and_keep_page_anchors() {
        for url in [
            "https://www.onenote.com/page?%61ccess_token=placeholder",
            "https://www.onenote.com/page#id_token=placeholder",
            "https://www.onenote.com/page#?code=placeholder",
        ] {
            assert!(microsoft_web_url(url).is_none());
        }
        let page = "https://www.onenote.com/page#section-id";
        assert_eq!(microsoft_web_url(page).as_deref(), Some(page));
    }

    #[test]
    fn reviewed_action_matches_legacy_title_and_keeps_unknown_attempt_blocked() {
        let mut ledger = ExportLedger::new("meeting");
        let scope = "todo:tenant:user:list";
        let previous = "todo:tenant:user:list:aaaaaaaaaaaaaaaa:action-1:bbbbbbbbbbbbbbbb";
        ledger.record_failure(previous, ExportStatus::UnknownAfterSubmit, None, None);
        let selected = super::super::actions::LegacyAction {
            id: "action-1".into(),
            title_hash: "cccccccccccccccc".into(),
        };
        let id = uuid::Uuid::new_v4().to_string();
        let restored =
            ledger.existing_task_key(scope, "meeting", &id, "bbbbbbbbbbbbbbbb", Some(&selected));
        assert_eq!(restored.as_deref(), Some(previous));
        assert!(!ledger.may_attempt(restored.as_deref().unwrap()));
        assert!(ledger
            .existing_task_key(
                "todo:tenant:other:list",
                "meeting",
                &id,
                "bbbbbbbbbbbbbbbb",
                Some(&selected)
            )
            .is_none());
    }

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
