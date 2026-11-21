# Microsoft To Do Export

ClawScribe exports reviewed action items to Microsoft To Do through Microsoft
Graph delegated auth. To Do is personal task capture, so the export creates
tasks only in the signed-in user's selected To Do list.

## Flow

1. User signs in to Microsoft 365.
2. User chooses a To Do list in Settings → Microsoft 365.
3. User opens To Do export from a meeting summary.
4. ClawScribe loads open reviewed meeting actions or suggests tasks from the summary.
   **Review suggestions** opens the action list when the summary contains
   additional suggestions. Review and save any additions before reopening the export.
   Structured suggestions can be added here. Checked-off fallback actions are
   excluded from the export preview.
5. User edits titles/notes and deselects anything that should not be created.
6. ClawScribe creates the selected tasks through Microsoft Graph.

Nothing is created until the user confirms the review dialog.

## Scope

Task list discovery and task creation use the delegated `Tasks.ReadWrite` scope,
which is already required by Planner task export. No mail, file, or directory
scope is required.

## Destination

To Do export requires a list ID chosen in Settings → Microsoft 365. The saved list ID
and display name are non-sensitive destination metadata stored in localStorage.
If the list disappears or access changes, Graph will return a destination error
and the user can pick another list.

## Task Shape

Each selected action item becomes a To Do task:

- `title`: reviewed task title
- `body`: reviewed notes plus meeting/source context
- `dueDateTime`: set only when the action item has a plain `YYYY-MM-DD` due date

Owner names from the summary remain a note hint. To Do export does not map
owners to Azure AD users or assign tasks to other people.

## Duplicate Protection

Microsoft To Do task creation is not naturally idempotent. ClawScribe keeps a
local history keyed by account, list, meeting, and saved action ID. Re-exporting
the same saved action reuses that task ID even when its title or the selected
batch changes. Notes and due dates are checkpointed individually. If a field
still needs repair, the app reads the task and fills the missing value. Any
value on the task is preserved. Older confirmed tasks get the same check.
Ambiguous matches with older suggestions are held for review.

The ledger saves each pending attempt before contacting Graph and each result
afterwards. Unreadable history or a failed write stops export. If a submission
has an unknown outcome, inspect the To Do list and record the result in
**Export history** before retrying. See
[export history and recovery](microsoft-graph.md#export-history-and-recovery).
