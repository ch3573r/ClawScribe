//! Microsoft Graph export integration (calendar + OneNote + Planner).
//!
//! This module owns delegated Microsoft sign-in, token storage, Graph request
//! transport, calendar lookup, OneNote export, Planner export, sanitized error
//! mapping, and local duplicate-protection metadata. See:
//!
//! - `docs/integrations/microsoft-graph.md`
//! - `docs/integrations/onenote-export.md`
//! - `docs/integrations/planner-export.md`

pub mod auth;
pub mod client;
pub mod commands;
pub mod calendar;
pub mod confluence;
pub mod discovery;
pub mod error;
pub mod exporter;
pub mod interactive_auth;
pub mod ledger;
pub mod markdown_notes;
pub mod model;
pub mod ms_auth_state;
pub mod onenote;
pub mod planner;
pub mod reqwest_transport;
pub mod token_store;
pub mod transport;

pub use error::GraphErrorKind;
pub use exporter::{export_onenote, export_planner, ExportContext, ExportReport, OneNoteTarget};
pub use ledger::ExportLedger;
pub use model::{ExportActionItem, ExportDecision, ExportStatus, MeetingExport, MicrosoftConnectionState};
pub use planner::PlannerDestination;

use crate::summary::codex_provider::MeetingNotesOutput;

/// Build a [`MeetingExport`] from a summary provider's structured output.
///
/// This is the bridge from the existing summary pipeline into export: any
/// provider that yields a [`MeetingNotesOutput`] can be exported. Action items
/// receive deterministic local ids if they don't already have them.
pub fn meeting_export_from_notes(
    meeting_id: impl Into<String>,
    title: impl Into<String>,
    created_at: Option<String>,
    notes: &MeetingNotesOutput,
    transcript_excerpt: Option<String>,
) -> MeetingExport {
    let decisions = notes
        .decisions
        .iter()
        .map(|d| ExportDecision {
            decision: d.decision.clone(),
            owner: d.owner.clone(),
        })
        .collect();

    let mut action_items: Vec<ExportActionItem> = notes
        .action_items
        .iter()
        .map(|a| ExportActionItem {
            local_action_id: String::new(),
            task: a.task.clone(),
            owner: a.owner.clone(),
            due_date: a.due_date.clone(),
            details: None,
        })
        .collect();
    planner::ensure_local_action_ids(&mut action_items);

    MeetingExport {
        meeting_id: meeting_id.into(),
        title: title.into(),
        created_at,
        executive_summary: notes.executive_summary.clone(),
        decisions,
        action_items,
        transcript_excerpt,
        summary_html: None,
    }
}
