//! `exports.json` idempotency ledger.
//!
//! Lives in app data and records, per dedupe key, what has already
//! been exported so a succeeded page/task is never created twice. It contains
//! destination and resource IDs, display names, URLs, timestamps, statuses,
//! and sanitized error codes. It contains no tokens. See the idempotency
//! sections of the OneNote/Planner design docs.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::exports::model::ExportStatus;

pub const LEDGER_SCHEMA: &str = "clawscribe.exports.v1";
pub const LEDGER_FILENAME: &str = "exports.json";
pub(crate) const MAX_LEDGER_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AmbiguousTaskIdentity;

/// Original title hash → positional IDs in the complete current candidate set.
pub type PositionalSiblings = HashMap<String, HashSet<String>>;

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

pub(super) fn is_positional_action_id(action_id: &str) -> bool {
    action_id
        .strip_prefix("action-")
        .is_some_and(|position| position.parse::<u32>().is_ok())
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

/// Hash composite destinations without storing server origins or folder names.
pub(super) fn artifact_key_hash(parts: &[&str]) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for part in parts {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

pub(super) fn task_key_for_action(
    scope: &str,
    meeting_id: &str,
    action: &super::model::ExportActionItem,
    title_hash: &str,
) -> String {
    let original_hash = action
        .legacy_action
        .as_ref()
        .map_or(title_hash, |legacy| legacy.title_hash.as_str());
    task_key(scope, meeting_id, &action.local_action_id, original_hash)
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
    /// Absent in older task entries. Read the saved remote task before repairing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_enrichment: Option<TaskEnrichment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskEnrichment {
    pub notes_pending: bool,
    pub due_date_pending: bool,
}

impl TaskEnrichment {
    pub fn is_pending(self) -> bool {
        self.notes_pending || self.due_date_pending
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes_page_id: Option<String>,
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
            task_enrichment: None,
            display_name: None,
        }
    }
}

/// Merge priority for export statuses. Higher wins over lower when merging two
/// ledgers. Pending must be normalised to UnknownAfterSubmit before calling this.
fn status_merge_priority(status: ExportStatus) -> u8 {
    match status {
        ExportStatus::Succeeded => 4,
        ExportStatus::UnknownAfterSubmit => 3,
        ExportStatus::DestinationNotFound
        | ExportStatus::AccessDenied
        | ExportStatus::TenantBlocked => 2,
        ExportStatus::Failed | ExportStatus::PartialFailure | ExportStatus::FailedAuth => 1,
        ExportStatus::Pending => 0,
    }
}

/// Return the later of two optional RFC-3339 timestamps (lexicographic order is
/// correct for same-precision, same-timezone RFC-3339 strings).
fn max_opt_timestamp(a: Option<&str>, b: Option<&str>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if a >= b { a } else { b }.to_string()),
        (Some(a), None) => Some(a.to_string()),
        (None, Some(b)) => Some(b.to_string()),
        (None, None) => None,
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
        for (key, entry) in &mut ledger.entries {
            if matches!(key.split(':').next(), Some("planner" | "todo"))
                && entry.resource_id.is_some()
                && entry.status.allows_automatic_retry()
            {
                // Older writers could record a failed follow-up with a known task ID.
                entry.status = ExportStatus::Succeeded;
            } else if entry.status == ExportStatus::Pending {
                entry.status = ExportStatus::UnknownAfterSubmit;
                entry.code = Some("interrupted_export_review_required".into());
            } else if entry.status == ExportStatus::Succeeded
                && entry.resource_id.is_none()
                && matches!(
                    key.split(':').next(),
                    Some("onenote" | "planner" | "todo" | "onedrive" | "confluence")
                )
            {
                entry.status = ExportStatus::UnknownAfterSubmit;
                entry.code = Some("remote_id_unconfirmed".into());
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
                ["onedrive", destination, revision, "folder" | "docx" | "pdf" | "docx-link" | "pdf-link"] => {
                    hash(destination, 64) && hash(revision, 64)
                }
                ["confluence", destination] => hash(destination, 64),
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
                        .display_name
                        .as_deref()
                        .is_some_and(|name| !text(name, 1024))
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
                    || section
                        .notes_page_id
                        .as_deref()
                        .is_some_and(|id| !valid_remote_id(id))
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

    /// Merge entries from `incoming` into `self`.
    ///
    /// Used when restoring into an installation that already has a ledger file —
    /// for example after a previous partial restore left one behind.
    ///
    /// Rules:
    /// * `Succeeded` beats everything; statuses that block automatic retry beat
    ///   those that allow it; `Pending` is normalised to `UnknownAfterSubmit`.
    /// * When both entries have succeeded with *different* `resource_id`s the
    ///   existing entry is kept intact (a code is added to signal the conflict)
    ///   rather than silently overwriting the local machine's known remote ID.
    /// * `resource_id`: on conflict (both set, different), keep existing.
    /// * `attempts`: take the maximum of both.
    /// * `onenote_sections`: add incoming sections not already present.
    pub(crate) fn merge_incoming(&mut self, incoming: &ExportLedger) {
        for (key, inc) in &incoming.entries {
            let inc_status = if inc.status == ExportStatus::Pending {
                ExportStatus::UnknownAfterSubmit
            } else {
                inc.status
            };

            match self.entries.entry(key.clone()) {
                std::collections::btree_map::Entry::Vacant(v) => {
                    let mut entry = inc.clone();
                    entry.status = inc_status;
                    if inc.status == ExportStatus::Pending && entry.code.is_none() {
                        entry.code = Some("interrupted_export_review_required".into());
                    }
                    v.insert(entry);
                }
                std::collections::btree_map::Entry::Occupied(mut o) => {
                    let ex = o.get_mut();
                    // Normalise existing pending status.
                    if ex.status == ExportStatus::Pending {
                        ex.status = ExportStatus::UnknownAfterSubmit;
                        if ex.code.is_none() {
                            ex.code = Some("interrupted_export_review_required".into());
                        }
                    }

                    // Both succeeded with different resource IDs: genuinely contradictory.
                    // Preserve the existing entry intact and flag for user review; do not
                    // overwrite the local machine's known remote ID.
                    let resource_conflict = ex.resource_id.is_some()
                        && inc.resource_id.is_some()
                        && ex.resource_id != inc.resource_id
                        && ex.status.is_terminal_success()
                        && inc_status.is_terminal_success();
                    if resource_conflict {
                        ex.code =
                            Some("merge_conflict_check_for_duplicates_in_microsoft_365".into());
                        ex.attempts = ex.attempts.max(inc.attempts);
                        continue;
                    }

                    let prefer_incoming =
                        status_merge_priority(inc_status) > status_merge_priority(ex.status);

                    // resource_id: on a non-contradictory conflict (at most one side has an
                    // ID), use whichever the winning side provides, falling back to the other.
                    let new_resource_id = if ex.resource_id.is_some()
                        && inc.resource_id.is_some()
                        && ex.resource_id != inc.resource_id
                    {
                        // Single-side contradiction: keep existing.
                        ex.resource_id.clone()
                    } else if prefer_incoming {
                        inc.resource_id.clone().or_else(|| ex.resource_id.clone())
                    } else {
                        ex.resource_id.clone().or_else(|| inc.resource_id.clone())
                    };

                    if ex.resource_id == inc.resource_id && !resource_conflict {
                        // A completed step stays completed when an older snapshot is restored.
                        ex.task_enrichment = match (ex.task_enrichment, inc.task_enrichment) {
                            (Some(local), Some(incoming)) => Some(TaskEnrichment {
                                notes_pending: local.notes_pending && incoming.notes_pending,
                                due_date_pending: local.due_date_pending
                                    && incoming.due_date_pending,
                            }),
                            (local, incoming) => local.or(incoming),
                        };
                    } else if ex.resource_id.is_none() && inc.resource_id.is_some() {
                        ex.task_enrichment = inc.task_enrichment;
                    }

                    if prefer_incoming {
                        ex.status = inc_status;
                        ex.code = inc.code.clone();
                        ex.web_url = inc.web_url.clone().or_else(|| ex.web_url.clone());
                        ex.last_attempt_at = max_opt_timestamp(
                            ex.last_attempt_at.as_deref(),
                            inc.last_attempt_at.as_deref(),
                        );
                    } else {
                        ex.web_url = ex.web_url.clone().or_else(|| inc.web_url.clone());
                    }
                    ex.resource_id = new_resource_id;
                    if ex.display_name.is_none() {
                        ex.display_name.clone_from(&inc.display_name);
                    }
                    ex.attempts = ex.attempts.max(inc.attempts);
                }
            }
        }

        // Merge onenote_sections: add incoming entries not already present locally.
        for (id, section) in &incoming.onenote_sections {
            self.onenote_sections
                .entry(id.clone())
                .and_modify(|saved| {
                    if saved.section_id.is_none()
                        && saved.tenant_id == section.tenant_id
                        && saved.user_id == section.user_id
                        && saved.notebook_id == section.notebook_id
                        && saved.content_hash == section.content_hash
                        && saved.section_name == section.section_name
                        && saved.page_title == section.page_title
                        && saved.options == section.options
                    {
                        saved.section_id.clone_from(&section.section_id);
                    }
                    if saved.notes_page_id.is_none()
                        && saved.section_id.is_some()
                        && saved.section_id == section.section_id
                        && saved.tenant_id == section.tenant_id
                        && saved.user_id == section.user_id
                        && saved.content_hash == section.content_hash
                    {
                        saved.notes_page_id.clone_from(&section.notes_page_id);
                    }
                })
                .or_insert_with(|| section.clone());
        }
    }

    /// Resolve past task attempts within this destination and meeting. Older
    /// keys use a batch hash and are already scoped to a meeting by the ledger.
    ///
    /// A cross-positional match can be a shift only if the complete candidate
    /// set is available and the matched ID and hash do not belong to a sibling.
    pub(crate) fn existing_task_key(
        &self,
        scope: &str,
        meeting_id: &str,
        action_id: &str,
        title_hash: &str,
        legacy_action: Option<&super::actions::LegacyAction>,
        full_positional_siblings: Option<&PositionalSiblings>,
    ) -> Result<Option<String>, AmbiguousTaskIdentity> {
        let current = task_key(scope, meeting_id, action_id, title_hash);
        let stable_id = uuid::Uuid::parse_str(action_id).is_ok();
        let mut exact_uuid = self.entries.get_key_value(&current).filter(|_| stable_id);
        if exact_uuid.is_some_and(|(_, entry)| {
            entry.resource_id.is_some() || !entry.status.allows_automatic_retry()
        }) {
            return Ok(Some(current));
        }

        let (old_id, old_hash) = legacy_action.map_or((action_id, title_hash), |legacy| {
            (&legacy.id, &legacy.title_hash)
        });
        let scope_prefix = format!("{scope}:");
        let v2_prefix = format!("v2:{}:", meeting_key_hash(meeting_id));
        let positional = is_positional_action_id(old_id);
        let candidates = self.entries.iter().filter_map(|(key, entry)| {
            let scoped = key.strip_prefix(&scope_prefix)?;
            let action = match scoped.strip_prefix(&v2_prefix) {
                Some(action) => action,
                None => {
                    let (batch, action) = scoped.split_once(':')?;
                    if batch.len() != 16 || !batch.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                        return None;
                    }
                    action
                }
            };
            let (id, hash) = action.split_once(':')?;
            (hash == old_hash || hash == title_hash).then_some((id, hash, key, entry))
        });
        fn priority(entry: &LedgerEntry) -> (bool, bool, Option<&str>) {
            (
                entry.status.is_terminal_success() || entry.resource_id.is_some(),
                !entry.status.allows_automatic_retry(),
                entry.last_attempt_at.as_deref(),
            )
        }
        let mut legacy_match = None;
        let mut ambiguous = false;
        for (id, hash, key, entry) in candidates {
            if stable_id && id == action_id {
                // Pre-v2 keys could also carry a UUID and title hash.
                if exact_uuid.is_none_or(|(_, previous)| priority(entry) >= priority(previous)) {
                    exact_uuid = Some((key, entry));
                }
            } else if id == old_id || (positional && is_positional_action_id(id)) {
                if id != old_id
                    && full_positional_siblings.is_none_or(|siblings| {
                        siblings.get(hash).is_some_and(|ids| ids.contains(id))
                    })
                {
                    ambiguous = true;
                    continue;
                }
                if let Some((previous_id, previous_hash, _, previous)) = legacy_match {
                    ambiguous |= previous_id != id || previous_hash != hash;
                    if priority(entry) < priority(previous) {
                        continue;
                    }
                }
                legacy_match = Some((id, hash, key, entry));
            }
        }

        // A retryable UUID can coexist with older protected history after a
        // restore. Keep that protection for suggested actions linked to it.
        if let Some((key, entry)) = exact_uuid {
            if entry.resource_id.is_some()
                || !entry.status.allows_automatic_retry()
                || (!ambiguous
                    && legacy_match.is_none_or(|(_, _, _, old)| {
                        old.resource_id.is_none() && old.status.allows_automatic_retry()
                    }))
            {
                return Ok(Some(key.clone()));
            }
        }
        if ambiguous {
            return Err(AmbiguousTaskIdentity);
        }
        Ok(legacy_match.map(|(_, _, key, _)| key.clone()))
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
            Some(e) if e.status.is_terminal_success() || e.resource_id.is_some() => false,
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

    pub(super) fn label_attempt(&mut self, dedupe_key: &str, name: &str) {
        if let Some(entry) = self.entries.get_mut(dedupe_key) {
            let label: String = name
                .trim()
                .chars()
                .filter(|ch| !ch.is_control())
                .take(240)
                .collect();
            entry.display_name = (!label.is_empty()).then_some(label);
        }
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

    pub(super) fn record_task_enrichment(
        &mut self,
        dedupe_key: &str,
        enrichment: TaskEnrichment,
        code: Option<String>,
    ) {
        if let Some(entry) = self.entries.get_mut(dedupe_key) {
            entry.task_enrichment = Some(enrichment);
            entry.code = code;
        }
    }

    /// Apply a user's explicit destination check. No remote write happens here.
    pub(super) fn reconcile(
        &mut self,
        key: &str,
        resource_id: Option<String>,
        now: Option<String>,
    ) -> Result<(), String> {
        if let Some(id) = &resource_id {
            if !valid_remote_id(id) {
                return Err("Enter a valid ID for the existing item.".into());
            }
        }
        if let Some(section) = key.strip_prefix("onenote-section:") {
            let entry = self
                .onenote_sections
                .get_mut(section)
                .filter(|entry| entry.section_id.is_none())
                .ok_or("This section no longer needs reconciliation.")?;
            if resource_id.is_none() {
                self.onenote_sections.remove(section);
            } else {
                entry.section_id = resource_id;
            }
            return Ok(());
        }
        let entry = self
            .entries
            .get_mut(key)
            .filter(|entry| entry.status == ExportStatus::UnknownAfterSubmit)
            .ok_or("This export no longer needs reconciliation.")?;
        if resource_id.is_some() {
            if entry.resource_id != resource_id {
                entry.task_enrichment = None;
                entry.web_url = None;
            }
            entry.status = ExportStatus::Succeeded;
            entry.resource_id = resource_id;
            entry.code = None;
        } else {
            entry.status = ExportStatus::Failed;
            entry.resource_id = None;
            entry.web_url = None;
            entry.task_enrichment = None;
            entry.code = Some("absence_confirmed".into());
        }
        entry.last_attempt_at = now;
        Ok(())
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

    #[tokio::test]
    async fn unknown_outcomes_stay_blocked_until_reconciled_and_survive_restart() {
        let folder = tempfile::tempdir().unwrap();
        let mut ledger = ExportLedger::load_or_new(folder.path(), "meeting").unwrap();
        let page = format!("confluence:{}", artifact_key_hash(&["example.com", "TEST"]));
        let absent = format!(
            "confluence:{}",
            artifact_key_hash(&["example.com", "OTHER"])
        );
        for key in [&page, &absent] {
            ledger.begin_attempt(key, None);
        }
        ledger.checkpoint().await.unwrap();
        let mut ledger = ExportLedger::load_or_new(folder.path(), "meeting").unwrap();
        assert!(!ledger.may_attempt(&page));
        assert!(!ledger.may_attempt(&absent));
        assert!(ledger
            .reconcile(&page, Some("../bad".into()), None)
            .is_err());
        ledger
            .reconcile(&page, Some("239812".into()), None)
            .unwrap();
        ledger.reconcile(&absent, None, None).unwrap();
        ledger.checkpoint().await.unwrap();
        let mut ledger = ExportLedger::load_or_new(folder.path(), "meeting").unwrap();
        assert_eq!(
            ledger
                .already_succeeded(&page)
                .unwrap()
                .resource_id
                .as_deref(),
            Some("239812")
        );
        assert!(!ledger.may_attempt(&page));
        assert!(ledger.may_attempt(&absent));
        assert!(ledger.reconcile(&page, None, None).is_err());
    }

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
        let restored = ledger
            .existing_task_key(
                scope,
                "meeting",
                &id,
                "bbbbbbbbbbbbbbbb",
                Some(&selected),
                Some(&PositionalSiblings::new()),
            )
            .unwrap();
        assert_eq!(restored.as_deref(), Some(previous));
        assert!(!ledger.may_attempt(restored.as_deref().unwrap()));
        assert!(ledger
            .existing_task_key(
                "todo:tenant:other:list",
                "meeting",
                &id,
                "bbbbbbbbbbbbbbbb",
                Some(&selected),
                Some(&PositionalSiblings::new()),
            )
            .unwrap()
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

    // ── merge_incoming ──────────────────────────────────────────────────────

    #[test]
    fn merge_adds_new_incoming_entries_and_normalises_pending() {
        let mut existing = ExportLedger::new("m");
        existing.record_failure("k-fail", ExportStatus::Failed, None, None);

        let mut incoming = ExportLedger::new("m");
        incoming.record_success("k-new", Some("id-new".into()), None, Some("t1".into()));
        incoming.begin_attempt("k-pending", None); // should become UnknownAfterSubmit

        existing.merge_incoming(&incoming);

        assert_eq!(
            existing.entry("k-new").unwrap().status,
            ExportStatus::Succeeded
        );
        assert_eq!(
            existing.entry("k-new").unwrap().resource_id.as_deref(),
            Some("id-new")
        );
        // Incoming pending normalised to unknown
        assert_eq!(
            existing.entry("k-pending").unwrap().status,
            ExportStatus::UnknownAfterSubmit
        );
        assert!(existing
            .entry("k-pending")
            .unwrap()
            .code
            .as_deref()
            .is_some());
    }

    #[test]
    fn merge_prefers_succeeded_over_failed_on_same_key() {
        let mut existing = ExportLedger::new("m");
        existing.record_failure("k", ExportStatus::Failed, Some("throttled".into()), None);
        existing.entries.get_mut("k").unwrap().attempts = 3;

        let mut incoming = ExportLedger::new("m");
        incoming.record_success("k", Some("task-1".into()), None, Some("t1".into()));
        incoming.entries.get_mut("k").unwrap().attempts = 1;

        existing.merge_incoming(&incoming);

        let e = existing.entry("k").unwrap();
        assert_eq!(e.status, ExportStatus::Succeeded);
        assert_eq!(e.resource_id.as_deref(), Some("task-1"));
        assert_eq!(e.attempts, 3); // max
    }

    #[test]
    fn merge_keeps_unknown_over_incoming_failed() {
        let mut existing = ExportLedger::new("m");
        existing.record_failure("k", ExportStatus::UnknownAfterSubmit, None, None);

        let mut incoming = ExportLedger::new("m");
        incoming.record_failure("k", ExportStatus::Failed, Some("timeout".into()), None);

        existing.merge_incoming(&incoming);

        let e = existing.entry("k").unwrap();
        assert_eq!(e.status, ExportStatus::UnknownAfterSubmit);
    }

    #[test]
    fn merge_contradictory_succeeded_resource_ids_flags_and_keeps_existing() {
        let mut existing = ExportLedger::new("m");
        existing.record_success("k", Some("task-local".into()), None, None);

        let mut incoming = ExportLedger::new("m");
        incoming.record_success("k", Some("task-archive".into()), None, None);

        existing.merge_incoming(&incoming);

        let e = existing.entry("k").unwrap();
        // Status unchanged (still Succeeded), local resource_id preserved
        assert_eq!(e.status, ExportStatus::Succeeded);
        assert_eq!(e.resource_id.as_deref(), Some("task-local"));
        // Conflict flagged via code
        assert!(e.code.as_ref().is_some_and(|c| c.contains("conflict")));
    }

    #[test]
    fn merge_onenote_sections_adds_incoming_keeps_existing() {
        let section_a = OneNoteSection {
            tenant_id: "t".into(),
            user_id: "u".into(),
            notebook_id: "nb".into(),
            section_id: Some("s1".into()),
            section_name: "Existing".into(),
            page_title: "p".into(),
            content_hash: "a".repeat(64),
            options: None,
            started_at: "2026-11-01T00:00:00Z".into(),
            notes_page_id: None,
        };
        let section_b = OneNoteSection {
            section_name: "Incoming".into(),
            section_id: Some("s2".into()),
            ..section_a.clone()
        };
        let section_c = OneNoteSection {
            section_name: "New".into(),
            section_id: Some("s3".into()),
            ..section_a.clone()
        };

        let uuid_a = "00000000-0000-0000-0000-000000000001";
        let uuid_b = "00000000-0000-0000-0000-000000000002";

        let mut existing = ExportLedger::new("m");
        existing
            .onenote_sections
            .insert(uuid_a.into(), section_a.clone());

        let mut incoming = ExportLedger::new("m");
        incoming.onenote_sections.insert(uuid_a.into(), section_b); // conflict — keep existing
        incoming
            .onenote_sections
            .insert(uuid_b.into(), section_c.clone()); // new — add

        existing.merge_incoming(&incoming);

        assert_eq!(existing.onenote_sections[uuid_a].section_name, "Existing");
        assert_eq!(existing.onenote_sections[uuid_b].section_name, "New");
        existing
            .onenote_sections
            .get_mut(uuid_b)
            .unwrap()
            .section_id = None;
        existing.merge_incoming(&incoming);
        assert_eq!(
            existing.onenote_sections[uuid_b].section_id.as_deref(),
            Some("s3")
        );
    }

    // ── existing_task_key positional dedup ──────────────────────────────────

    #[test]
    fn positional_dedup_finds_same_title_at_different_action_n_position() {
        let mut ledger = ExportLedger::new("meeting");
        let scope = "planner:tenant:user:plan:bucket";
        let hash_a = "aaaa000000000000";
        // A was at action-1
        let key_a1 = task_key(scope, "meeting", "action-1", hash_a);
        ledger.record_success(&key_a1, Some("task-A".into()), None, None);

        // Now A is at action-2 (new item inserted before it)
        let found = ledger
            .existing_task_key(
                scope,
                "meeting",
                "action-2",
                hash_a,
                None,
                Some(&PositionalSiblings::new()),
            )
            .unwrap();
        assert_eq!(found.as_deref(), Some(key_a1.as_str()));
        assert_eq!(
            ledger
                .already_succeeded(&key_a1)
                .unwrap()
                .resource_id
                .as_deref(),
            Some("task-A")
        );
    }

    #[test]
    fn positional_dedup_requires_review_when_ambiguous() {
        let mut ledger = ExportLedger::new("meeting");
        let scope = "planner:tenant:user:plan:bucket";
        let hash = "bbbb000000000000";
        // Same title at two positions
        ledger.record_success(
            &task_key(scope, "meeting", "action-1", hash),
            None,
            None,
            None,
        );
        ledger.record_success(
            &task_key(scope, "meeting", "action-3", hash),
            None,
            None,
            None,
        );

        // action-2 with same hash — ambiguous, must not match either
        let found = ledger.existing_task_key(
            scope,
            "meeting",
            "action-2",
            hash,
            None,
            Some(&PositionalSiblings::new()),
        );
        assert_eq!(found, Err(AmbiguousTaskIdentity));
    }

    #[test]
    fn positional_dedup_skipped_for_uuid_action_id() {
        let mut ledger = ExportLedger::new("meeting");
        let scope = "planner:tenant:user:plan:bucket";
        let hash = "cccc000000000000";
        // A UUID entry already exists (no title_hash in key)
        let uuid = uuid::Uuid::new_v4().to_string();
        ledger.record_success(
            &task_key(scope, "meeting", &uuid, hash),
            Some("task-UUID".into()),
            None,
            None,
        );
        // Positional search for action-N should not find the UUID entry
        let found = ledger
            .existing_task_key(
                scope,
                "meeting",
                "action-1",
                hash,
                None,
                Some(&PositionalSiblings::new()),
            )
            .unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn positional_dedup_distinct_titles_dont_interfere() {
        let mut ledger = ExportLedger::new("meeting");
        let scope = "todo:tenant:user:list";
        let hash_a = "dddd000000000000";
        let hash_b = "eeee000000000000";
        ledger.record_success(
            &task_key(scope, "meeting", "action-1", hash_a),
            Some("task-A".into()),
            None,
            None,
        );
        ledger.record_success(
            &task_key(scope, "meeting", "action-2", hash_b),
            Some("task-B".into()),
            None,
            None,
        );

        // A shifts to action-2 (new item B shifts to action-3)
        let found_a = ledger
            .existing_task_key(
                scope,
                "meeting",
                "action-2",
                hash_a,
                None,
                Some(&PositionalSiblings::new()),
            )
            .unwrap();
        assert_eq!(
            found_a.as_deref(),
            Some(task_key(scope, "meeting", "action-1", hash_a).as_str())
        );

        // B shifts to action-3
        let found_b = ledger
            .existing_task_key(
                scope,
                "meeting",
                "action-3",
                hash_b,
                None,
                Some(&PositionalSiblings::new()),
            )
            .unwrap();
        assert_eq!(
            found_b.as_deref(),
            Some(task_key(scope, "meeting", "action-2", hash_b).as_str())
        );
    }

    #[test]
    fn positional_dedup_matches_old_format_key() {
        let mut ledger = ExportLedger::new("meeting");
        let scope = "planner:tenant:user:plan:bucket";
        let hash = "ffff000000000000";
        // Insert an old-format key (batch hash instead of v2)
        let old_key = format!("{scope}:1234567890abcdef:action-1:{hash}");
        ledger.record_success(&old_key, Some("task-old".into()), None, None);

        // action-2 lookup should find the old-format action-1 entry
        let found = ledger
            .existing_task_key(
                scope,
                "meeting",
                "action-2",
                hash,
                None,
                Some(&PositionalSiblings::new()),
            )
            .unwrap();
        assert_eq!(found.as_deref(), Some(old_key.as_str()));
    }

    #[test]
    fn matching_key_formats_preserve_unknown_outcome_and_destination_scope() {
        let mut ledger = ExportLedger::new("meeting");
        let scope = "todo:tenant:user:list";
        let hash = "ffff000000000000";
        let unknown = format!("{scope}:1234567890abcdef:action-1:{hash}");
        ledger.record_failure(&unknown, ExportStatus::UnknownAfterSubmit, None, None);
        for (scope, meeting_id, position) in [
            (scope, "meeting", "action-1"),
            ("todo:tenant:user:other", "meeting", "action-2"),
            (scope, "other", "action-3"),
        ] {
            ledger.record_failure(
                &task_key(scope, meeting_id, position, hash),
                ExportStatus::Failed,
                None,
                None,
            );
        }
        let found = ledger
            .existing_task_key(
                scope,
                "meeting",
                "action-4",
                hash,
                None,
                Some(&PositionalSiblings::new()),
            )
            .unwrap();
        assert_eq!(found.as_deref(), Some(unknown.as_str()));
        assert!(!ledger.may_attempt(&unknown));
    }
}
