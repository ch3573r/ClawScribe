# Microsoft Graph Integration

ClawScribe uses Microsoft Graph as an optional delegated integration for
calendar context, OneNote, Planner, Microsoft To Do, and OneDrive or SharePoint
file export. Microsoft auth is separate from OpenAI, Codex, Ollama, and
other summary providers.

## Current Capabilities

- Interactive Microsoft sign-in.
- Refresh-token storage through the platform credential store, with a
  current-user DPAPI-encrypted file fallback on Windows.
- Calendar read for current/next meeting context and invited attendees.
- OneNote notebook discovery, creation, export, and reviewed updates by saved ID.
- Planner plan/bucket discovery, optional bucket creation, task preview, and
  selected task export.
- Microsoft To Do list discovery, task preview, and selected personal task
  export.
- OneDrive or SharePoint destination selection and export of reviewed notes as Word or PDF.
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
- `Files.ReadWrite`
- `offline_access`

Scope rules:

- Do not add Graph scopes unless code calls the matching endpoint.
- Teams meeting detection is local process/window detection and does not require
  `OnlineMeetings.*` or `Presence.*`.
- File export uses `Files.ReadWrite` to write documents to the selected destination.
- Mail scopes should stay out unless an Outlook recap sender actually lands.
- `Notes.ReadWrite.All` is intentionally broader than basic page creation so
  shared-notebook workflows can work, but it should remain visible in reviews.

## Auth And Storage

The sign-in flow is foreground and user-initiated. The loopback listener checks
the callback route and sign-in state before accepting the authorization code.
Logs may record connection state and sanitized errors, but must not record
bearer tokens, refresh tokens, auth codes, or callback URLs containing credentials.

The persisted Microsoft session contains the refresh token and account metadata,
not the short-lived access token. When the Windows credential store cannot
persist it, the fallback file is protected with DPAPI for the current Windows
user. Successful migration removes the legacy plaintext fallback. If migration
fails, ClawScribe requires the user to sign in again.

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
and Confluence browser draft export must remain usable.

## Calendar

**Follow-up email** in the meeting's **Export** menu creates an editable draft
from open reviewed actions. A summary's email button starts with the provider's
draft. Attendees become eligible after you mark them present in **Attendance**.
Choose recipients for each draft and select **Open in email client**. ClawScribe
rechecks attendance and opens the operating system's compose handler. You send
from the email client. This flow uses local calendar snapshots and needs no
additional Graph permission. Use **Copy draft** if there is no default client,
an address format is unsupported, or the compose request is too long.

Calendar integration is read-only. It requests current/next meeting context and
the next event list, including:

- event ID
- subject
- online meeting metadata
- start/end
- organizer
- invited attendees
- event type and the series master ID for recurring occurrences and exceptions

The app uses a selected event for the next recording or links a single timed
event in progress across enabled sources. Upcoming events in the next five
minutes are offered for review. Competing matches stay unlinked until chosen.
The event body is excluded. The
meeting's attendance checklist starts unchecked and summaries receive only
calendar names confirmed in that checklist. Recording works without a link if
calendar data is unavailable. Calendar lookup uses the existing read permission.

Microsoft sign-out ends the connection and clears what came from the account
and was never confirmed: current event choices, an unreviewed selection and
automatically matched links on saved meetings. A link you reviewed is part of the
local meeting record and stays, with its attendance, until you unlink it.
iCalendar sources and their associations are unaffected.

For a saved recurring meeting, **Attendance → Template for this series** sets
the default summary template for linked occurrences and exceptions from the
same account. The preference stays on this device and is available when you
reconnect that account. The existing calendar read supplies the series master
ID. ClawScribe hashes it together with the sign-in authority and the account's
tenant-specific Graph user ID, and stores only that series key alongside the
existing meeting link. Single events, legacy links, and events without a usable
account or series ID use the regular summary template settings.
See [Calendar sources and attendance](icalendar.md).

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
- review and apply changes by saved page ID when the page has an editable export block
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
4. choose the destination To Do list in Settings → Microsoft 365
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

Choose **Export history** from a meeting’s export menu to see results and their
dates. **Open** is available when Microsoft supplied a usable link. History
includes Confluence direct publishing and travels with portable library backups.

OneNote, Planner, To Do, OneDrive, and Confluence record attempts before submitting
creates and save each result. Completed items survive interrupted batches. Planner
descriptions and To Do notes and due dates have separate completion state from
task creation. A matching export checks incomplete fields on the saved task and
fills missing values while preserving populated fields. Older task history with
a saved ID is checked the first time the task is re-exported.

OneDrive saves the Word file, PDF, and optional organization links independently.
Repeating the same content checks the confirmed files in the original folder and
uses the Word file for unfinished PDF conversion. If you link a file manually,
the review shows which file a repeat export will use for conversion and sharing.
New Word and PDF uploads use the same name before the extension, and retries keep
that name. Edited drafts get a dated filename. Legacy entries may require
reopening the original export with the same destination and content to restore
the recorded folder reference.

Unreadable or invalid history blocks export. A history write failure stops the
batch; it never silently disables duplicate protection. Restore access to the
existing history before retrying rather than deleting it.

If a response is lost, the app restarts during submission, or the result cannot
be saved locally, an item can have an **unknown outcome**. Inspect the original
destination, then choose **Resolve outcome**. For a saved item that has moved,
was deleted, or was linked incorrectly, choose **Review link**. Select the item
from the Microsoft list or paste the Confluence page address from **Page
information**. ClawScribe reads the candidate again under the original account
and checks the recorded destination before saving the link. A temporary service
failure leaves history intact. Microsoft candidate lists show up to 100 entries
and can be limited by service access or OneNote section listing limits.

After confirming an item is absent in the service, choose **Allow retry** where
available. A saved Microsoft item that can still be found or an inaccessible
original destination blocks this option. Resolve saved child items before
replacing their OneDrive folder or OneNote section. Saving a resolution changes
local history. A new export requires separate review and submission. Saved tasks
keep their identity across edits.

Confluence identifies the PAT account before publishing. History records a
fingerprint of the account and draft. Repeating a publish offers **Open** for
the existing page. If the local draft changed, review and apply those edits in
Confluence. A previously linked page might be hidden by a permissions change.
For an uncertain publish with a saved title, **Allow retry** verifies the space
and parent and searches the space for that title. A possible match or incomplete
search keeps the retry blocked. For older entries without a saved title, link
the page if you find it or review a browser draft before publishing manually.
Use the browser draft to review replacement of a deleted page as well.

Older Planner and To Do exports identified summary suggestions by title and
position. When a title matches several past tasks, or several suggestions match
one past task, ClawScribe marks the affected tasks for review. Check the selected
destination for existing tasks. Open **Meeting actions**, use **Add action** for
each task you confirm is new, and save those actions before exporting them.

OneNote saves the section ID before submitting pages. **Resume export** uses
the saved section and original content options. **Update exported page** opens
a review of the edited content and uses the saved page or section ID. Both
operations work without enumerating sections.

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

## Partial consent

An account can stay connected with only some permissions approved. Settings lists
unavailable exports and offers **Request missing permissions**. This opens one
explicit consent attempt. If the organization requires admin approval, the
existing session remains connected and approved exports remain usable. Missing
offline access means the session cannot be restored after closing the app.
Export and discovery commands check their required permissions before calling
Graph and report `Permission not granted: <scope>` when access is unavailable.
When a saved permission is missing and a refresh token is available, the command
refreshes once and checks again so later admin approval takes effect without
another interactive sign-in. A failed refresh preserves the existing session.
