# OneNote Export

ClawScribe exports meeting notes to OneNote through Microsoft Graph delegated
auth. The current product path intentionally avoids depending on section
listing because Microsoft Graph can fail section enumeration on large
OneDrive/SharePoint libraries.

## Current UX

1. User signs in to Microsoft.
2. User selects an existing notebook or creates a notebook.
3. Export creates a fresh dated section for the meeting.
4. ClawScribe writes editable OneNote pages into that section.
5. Local export metadata prevents accidental duplicate exports.

The section name is generated from the meeting title/date and sanitized for
OneNote constraints. This keeps exports predictable without requiring a section
picker.

## Why Section Listing Is Avoided

Graph can return:

```text
access_denied 10008:
One or more of the document libraries ... contains more than 5,000 OneNote items
```

That limit applies to the backing document library scan. A notebook with only a
few visible sections can still fail if the library contains too many OneNote
items overall. Filtering or `$expand=sections` does not reliably bypass the
scan.

Reliable operations:

- create a section in a known notebook
- create pages in a known section
- reuse a previously known section ID

Unreliable on over-limit libraries:

- list all sections for a notebook
- require section listing before export

## Graph Shape

Notebook discovery/creation and section/page creation use Microsoft Graph under
the signed-in user's delegated permissions.

Representative page create call:

```http
POST https://graph.microsoft.com/v1.0/me/onenote/sections/{section-id}/pages
Content-Type: application/xhtml+xml
Authorization: Bearer <access-token>
```

The payload is editable XHTML, not an image snapshot. That keeps exported notes
usable in OneNote.

## Page Content

The notes page should include:

- meeting title
- created/exported metadata
- invited-attendee checklist when available
- summary
- decisions
- action items
- transcript pages or transcript chunks when needed

OneNote HTML constraints:

- UTF-8 encoded XHTML
- supported semantic HTML only
- no JavaScript/forms
- no private local file paths unless explicitly intended
- transcript split across pages when payload size requires it

## Idempotency

OneNote page creation is not naturally idempotent. ClawScribe uses local export
metadata so retries do not blindly create duplicates.

The ledger may store:

- destination IDs
- page IDs / URLs
- sanitized error codes
- dedupe keys
- timestamps

It must not store access tokens, refresh tokens, auth codes, or raw auth URLs.

## Error Handling

- `401`: token expired or invalid; prompt reconnect.
- `403`: consent missing, tenant blocked, or access denied.
- `404`: notebook/section/page destination no longer visible.
- `413`: split pages or export summary-only.
- `429`: respect `Retry-After` and avoid duplicate page creation.
- `507`: target section is full; create/use another section.
- `10008`: do not retry section listing; use create-new/saved-ID flow.

Graph failures must leave local meeting artifacts intact.

## Review Rules

- Do not reintroduce required section listing.
- Do not silently export without user action.
- Do not log tokens or full auth callback URLs.
- Keep Microsoft auth independent from summary-provider auth.
- Keep notebook creation optional; exporting to an existing notebook must remain
  the normal path.
