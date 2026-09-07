# Microsoft Graph Integration

ClawScribe uses Microsoft Graph as an optional delegated integration for
calendar context, OneNote export, Planner task export, and Microsoft To Do task
export. Microsoft auth is separate from OpenAI, OpenClaw, Codex, Ollama, and
other summary providers.

## Current Capabilities

- Interactive Microsoft sign-in.
- Refresh-token storage through the platform credential store, with a
  current-user DPAPI-encrypted file fallback on Windows.
- Calendar read for current/next meeting context and invited attendees.
- OneNote notebook discovery/creation and export.
- Planner plan/bucket discovery, optional bucket creation, task preview, and
  selected task export.
- Microsoft To Do list discovery, task preview, and selected personal task
  export.
- Connection status surfaced in the integrations/settings UI.

## Delegated Scopes

The default scopes live in `frontend/src-tauri/src/exports/auth.rs`.

Current scope set:

- `User.Read`
- `Notes.ReadWrite`
- `Notes.Create`
- `Notes.ReadWrite.All`
- `Tasks.ReadWrite`
- `Calendars.Read`
- `offline_access`

Scope rules:

- Do not add Graph scopes unless code calls the matching endpoint.
- Teams meeting detection is local process/window detection and does not require
  `OnlineMeetings.*` or `Presence.*`.
- File access scopes should stay out unless a OneDrive/SharePoint file export
  feature actually lands.
- Mail scopes should stay out unless an Outlook recap sender actually lands.
- `Notes.ReadWrite.All` is intentionally broader than basic page creation so
  shared-notebook workflows can work, but it should remain visible in reviews.

## Auth And Storage

The sign-in flow is foreground and user-initiated. Logs may record connection
state and sanitized Graph errors, but must not record bearer tokens, refresh
tokens, auth codes, or callback URLs containing credentials.

The persisted Microsoft session contains the refresh token and account metadata,
not the short-lived access token. When the Windows credential store cannot
persist it, the fallback file is protected with DPAPI for the current Windows
user. A legacy plaintext fallback is removed only after successful encrypted
migration; failed migration can leave the legacy file in place. Non-Windows
platforms do not write a new plaintext Microsoft-token fallback.

AI and hosted-transcription keys use separate provider-specific protected
credential references; Microsoft sign-in does not supply those credentials.
Recordings and transcripts are not encrypted by credential storage. Treat
application data and backups as sensitive. See
[privacy and data handling](../../PRIVACY_POLICY.md).

Expected connection states:

- `not_connected`
- `connecting`
- `connected`
- `expired`
- `consent_required`
- `tenant_blocked`
- `access_denied`

If Microsoft auth fails, local recording, transcription, summary generation,
OpenClaw, and Confluence browser draft export must remain usable.

## Calendar

Calendar integration is read-only. It requests current/next meeting context and
the next event list, including:

- event ID
- subject
- online meeting metadata
- start/end
- organizer
- invited attendees

The app uses the selected event to seed the next recording title and to prepend
invited attendees to the generated summary as a checklist. The event body is not
read for summary context. Invited attendees are not proof of actual attendance.

## OneNote

The reliable OneNote path is create-by-name/create-by-ID, not section listing.
Graph can fail section enumeration with error `10008` when the backing
OneDrive/SharePoint library contains more than 5,000 OneNote items, even if the
selected notebook itself has only a few sections.

ClawScribe should therefore:

- list/create notebooks where supported
- create a fresh dated section for an export
- write notes/transcript pages under that section
- use a known saved section ID only when one is already available
- avoid making section listing a required export step

See [onenote-export.md](onenote-export.md).

## Planner

Planner export is review-first:

1. extract action items from the meeting notes
2. preview candidate tasks
3. let the user edit/deselect tasks
4. choose plans/buckets or create buckets
5. export selected tasks
6. record local duplicate-protection metadata

Tasks must not be silently created from AI output without user review.

See [planner-export.md](planner-export.md).

## Microsoft To Do

To Do export is also review-first:

1. extract action items from the meeting notes
2. preview candidate personal tasks
3. let the user edit/deselect tasks and notes
4. choose the destination To Do list in Settings → Add-ons
5. export selected tasks
6. record local duplicate-protection metadata

To Do uses the existing `Tasks.ReadWrite` scope and targets the signed-in user's
lists under `/me/todo/...`. It does not assign tasks to other users.

See [todo-export.md](todo-export.md).

## Error Handling

- `401`: mark session expired and prompt reconnect.
- `403`: show consent, tenant policy, or access-denied guidance.
- `404`: selected destination no longer exists or is not visible.
- `429`: respect `Retry-After` and avoid duplicate exports.
- `503`: bounded retry, then surface service unavailable.
- OneNote `10008`: skip section listing and use create-new/saved-ID flows.

Graph failures should not delete local meeting artifacts.

## Export History And Recovery

OneNote page and Planner/To Do task creation save a pending attempt locally before
contacting Graph, then save each result. Completed items survive an interrupted
batch and are skipped on a matching re-export. Exports run one at a time so
separate dialogs cannot overwrite each other's history.

Unreadable or invalid history blocks export. A history write failure stops the
batch; it never silently disables duplicate protection. Restore access to the
existing history before retrying rather than deleting it.

If a response is lost, the app restarts during submission, or a result cannot be
saved locally, an item can have an **unknown outcome** (`unknown_after_submit`).
ClawScribe cannot determine from that state whether Microsoft created it and
will not automatically submit that item again. Inspect the selected destination
for the page or task before taking further action. There is no automatic remote
reconciliation or reset control for these entries. Changing a title or destination
can create a different export identity and must not be used as a blind retry.

OneNote retains a newly created section if a page is confirmed or may have been
created. Duplicate protection applies to matching content and destinations; it
does not make Graph's create operations intrinsically idempotent.

## Test And Review Checklist

Before changing Microsoft integration:

- verify requested scopes match actual endpoint usage
- run relevant Rust tests under `exports::`
- smoke sign-in and connection status on Windows
- test OneNote export without relying on section listing
- test Planner preview before task creation
- test To Do preview before task creation
- verify no tokens or auth URLs appear in logs
- verify local recording and summary still work when Microsoft is disconnected
