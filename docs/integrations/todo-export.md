# Microsoft To Do Export

ClawScribe exports reviewed action items to Microsoft To Do through Microsoft
Graph delegated auth. To Do is personal task capture, so the export creates
tasks only in the signed-in user's selected To Do list.

## Flow

1. User signs in to Microsoft 365.
2. User chooses a To Do list in Settings → Add-ons.
3. User opens To Do export from a meeting summary.
4. ClawScribe parses the summary action items into a review dialog.
5. User edits titles/notes and deselects anything that should not be created.
6. ClawScribe creates the selected tasks through Microsoft Graph.

Nothing is created until the user confirms the review dialog.

## Scope

Task list discovery and task creation use the delegated `Tasks.ReadWrite` scope,
which is already required by Planner task export. No mail, file, or directory
scope is required.

## Destination

To Do export requires a list ID chosen in Settings → Add-ons. The saved list ID
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
local export ledger keyed by tenant, user, list, meeting artifact hash, action
ID, and reviewed title hash. Re-exporting the same reviewed task skips the
already-created task; editing the title creates a new task intentionally.
