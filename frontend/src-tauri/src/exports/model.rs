//! Shared types for Microsoft Graph export.
//!
//! These are deliberately decoupled from the summary providers: an exporter
//! consumes a [`MeetingExport`] (which can be built from any summary source)
//! rather than a provider-specific output struct. See
//! `docs/integrations/microsoft-graph.md` for the design.

use serde::{Deserialize, Serialize};

/// Connection state for the (separate) Microsoft account used for export.
///
/// This is independent from OpenAI / OpenClaw / Codex auth. Mirrors the states
/// documented in `microsoft-graph.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MicrosoftConnectionState {
    NotConnected,
    Connecting,
    Connected,
    ConsentRequired,
    TenantBlocked,
    AccessDenied,
    Expired,
}

/// Outcome of an export attempt for a single destination (or item).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportStatus {
    /// In-flight; a fresh attempt has been recorded but not yet completed.
    Pending,
    Succeeded,
    /// Generic failure with a sanitized error code recorded alongside.
    Failed,
    /// Token expired/invalid; the Microsoft connection should be reconnected.
    FailedAuth,
    /// Signed in but not authorized for the resource.
    AccessDenied,
    /// Tenant policy blocks user consent / the app.
    TenantBlocked,
    /// Stored destination ID is gone or not visible to the user.
    DestinationNotFound,
    /// Some items in a batch succeeded and some failed.
    PartialFailure,
    /// A create may or may not have landed after a network timeout. Requires
    /// user review before any retry to avoid duplicates.
    UnknownAfterSubmit,
}

impl ExportStatus {
    /// Whether an existing ledger entry with this status means "already done,
    /// do not call Graph again".
    pub fn is_terminal_success(self) -> bool {
        matches!(self, ExportStatus::Succeeded)
    }

    /// Whether a retry may be attempted automatically without user review.
    /// `UnknownAfterSubmit` and `DestinationNotFound` always need review first.
    pub fn allows_automatic_retry(self) -> bool {
        matches!(self, ExportStatus::Failed | ExportStatus::PartialFailure)
    }
}

/// A meeting's action item, normalized for export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportActionItem {
    /// Stable, deterministic identifier for this action within the meeting.
    /// Used as part of the Planner per-item dedupe key.
    pub local_action_id: String,
    /// Single-line task title.
    pub task: String,
    /// Optional free-text owner. Never auto-mapped to an Azure AD user.
    pub owner: Option<String>,
    /// Optional ISO-8601 due date, only when explicitly extracted/reviewed.
    pub due_date: Option<String>,
    /// Optional reviewed task notes (e.g. AI-polished). When set, used as the
    /// Planner task description instead of the templated default.
    #[serde(default)]
    pub details: Option<String>,
}

/// A decision captured in the meeting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportDecision {
    pub decision: String,
    pub owner: Option<String>,
}

/// The normalized meeting content an exporter consumes.
///
/// Build this from a [`crate::summary::codex_provider::MeetingNotesOutput`] or
/// any other summary source. It carries only export-relevant fields and never
/// holds tokens or credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingExport {
    /// Stable meeting identifier (e.g. a recording-folder slug).
    pub meeting_id: String,
    /// Human-friendly meeting title.
    pub title: String,
    /// ISO-8601 creation timestamp, if known.
    pub created_at: Option<String>,
    pub executive_summary: String,
    pub decisions: Vec<ExportDecision>,
    pub action_items: Vec<ExportActionItem>,
    /// Optional transcript text. May be split across OneNote pages.
    pub transcript_excerpt: Option<String>,
    /// Optional pre-rendered XHTML for the OneNote "Summary" section. When set,
    /// it is emitted verbatim (already sanitized) instead of escaping
    /// `executive_summary` into one paragraph — so a full markdown summary keeps
    /// its headings, lists, and emphasis on the exported page.
    #[serde(default)]
    pub summary_html: Option<String>,
}

impl MeetingExport {
    /// A stable content hash used in dedupe keys, so re-exporting identical
    /// content reuses the same ledger entry while edited content does not.
    pub fn artifact_hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(self.meeting_id.as_bytes());
        hasher.update(b"\x1f");
        hasher.update(self.title.as_bytes());
        hasher.update(b"\x1f");
        hasher.update(self.executive_summary.as_bytes());
        hasher.update(b"\x1f");
        for d in &self.decisions {
            hasher.update(d.decision.as_bytes());
            hasher.update(b"\x1e");
        }
        for a in &self.action_items {
            hasher.update(a.local_action_id.as_bytes());
            hasher.update(b"=");
            hasher.update(a.task.as_bytes());
            hasher.update(b"\x1e");
        }
        let digest = hasher.finalize();
        // Short, stable hex prefix is enough for a dedupe key component.
        digest[..8].iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_hash_is_stable_and_content_sensitive() {
        let base = MeetingExport {
            meeting_id: "m1".into(),
            title: "Weekly sync".into(),
            created_at: None,
            executive_summary: "Summary".into(),
            decisions: vec![],
            action_items: vec![ExportActionItem {
                local_action_id: "action-1".into(),
                task: "Send proposal".into(),
                owner: None,
                due_date: None,
                details: None,
            }],
            transcript_excerpt: None,
            summary_html: None,
        };

        let h1 = base.artifact_hash();
        let h2 = base.clone().artifact_hash();
        assert_eq!(h1, h2, "same content must hash identically");

        let mut edited = base.clone();
        edited.executive_summary = "Different".into();
        assert_ne!(h1, edited.artifact_hash(), "edited content must change hash");
    }

    #[test]
    fn status_retry_rules() {
        assert!(ExportStatus::Succeeded.is_terminal_success());
        assert!(!ExportStatus::Failed.is_terminal_success());
        assert!(ExportStatus::Failed.allows_automatic_retry());
        assert!(!ExportStatus::UnknownAfterSubmit.allows_automatic_retry());
        assert!(!ExportStatus::DestinationNotFound.allows_automatic_retry());
    }
}
