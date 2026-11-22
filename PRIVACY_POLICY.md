# ClawScribe Privacy And Data Handling

Updated 2026-11-21.

ClawScribe is a local-first meeting recorder and summarizer. Local recording and
local transcription do not require a ClawScribe account. Optional providers,
exports, model downloads, and update checks have separate network behavior.
This page describes the application paths; it does not make guarantees about
third-party providers, Windows, or a user's network and storage configuration.

## Local Meeting Data

In local recording/transcription mode, microphone and system audio are processed
on the device and saved to local recording files. Meeting metadata, transcripts,
settings, and summaries use local files and a database. Local summary providers
run on the device when their configured endpoint is local.

Local-first does not mean the application never makes a network request. Models
must be downloaded, update checks contact the configured release service, and
optional integrations make the requests described below. Disable or leave
unconfigured external meeting-processing features when meeting content must
remain on the device.

## Optional Off-Device Processing

**Hosted transcription.** Cloud transcription is opt-in and used only for Import
and Enhance (whole-file). A configured Hosted Whisper or MAI request uploads the
selected audio to its provider; local conversion can produce a WAV file for
upload. Live recordings always transcribe on the device, even with a cloud engine
saved. Provider retention and processing depend on the account, endpoint, and
third party's terms.

**Summary and meeting chat providers.** A remote provider receives transcript or
meeting text and supplied context needed for the selected operation. OpenAI,
OpenAI-compatible endpoints, Anthropic, Groq, OpenRouter, remote Ollama, and the
configured Codex path can send this data off-device. Ollama processing stays on
your device when its endpoint is local.

**Personal notes in a library question.** Searching personal notes and bookmarks
runs on your device. **Ask about these results** opens a question scoped to the
displayed meetings. To share a personal excerpt, select it, review the exact
text, provider, model and destination, then confirm the question. Approval
allows one send attempt. ClawScribe excludes that question and answer from
the history it sends with later questions. A new attempt requires a new review.

**Microsoft Graph.** Signing in exchanges credentials with Microsoft and enables
the delegated integration. Calendar features read meeting context and invited
attendees. User-selected exports send notes/transcripts or tasks to OneNote,
OneDrive, Planner, or Microsoft To Do under the signed-in account. Review the
selected content and task assignments before exporting. Microsoft sign-in is separate
from MAI transcription credentials and AI-provider credentials.

**Confluence.** A browser draft uses copied content in the target browser. Direct
publishing sends the chosen document to the configured Server/Data Center
endpoint. Review the destination and content before publishing.

**Outbound webhook.** The webhook is off by default. If you enable it, ClawScribe
sends meeting details to your saved endpoint when you export or when the
recording, import or summary events you select occur. You can choose whether
to include summary and action items or transcript text. Test deliveries use
sample data. The local delivery history stores the event, meeting ID, time,
attempt count and outcome. [Webhook configuration and payload](docs/integrations/webhook.md)

**Export history and recovery.** The local meeting history can contain task,
page, and file names, remote item and destination IDs, service links, status,
timestamps, and account and export revision fingerprints. It travels with
portable library backups. Recovery reads the original destination using the
saved connection to verify an item you choose. The candidate list is displayed
for review. Saving a resolution records the selected item or confirmed absence
locally. The OneNote page includes a content marker
derived from the meeting and revision so the app can identify its own block.
Dismissed suggestions keep an action ID and title fingerprint with the meeting
so the app can recognize them when offering suggestions again.

**Downloads and updates.** Model publishers, release hosting, and configured
provider services receive the network requests required for those operations.
Automatic update checks are not meeting-content uploads.

## Credential Storage

Summary and hosted-transcription API keys use Windows Credential Manager. If
that store is unavailable on Windows, a current-user DPAPI-encrypted fallback is
stored in SQLite. The app migrates legacy plaintext before use. If protected
storage or migration fails, the provider cannot use the saved secret until
protected storage is available. The old plaintext may remain pending migration.

The bundled Codex provider uses an isolated profile and keyring-only ChatGPT
sign-in, with no credential-file fallback. An old `auth.json` is removed after a
keyring sign-in succeeds. The Confluence token and its destination binding are
stored in the keyring.

The webhook URL and optional signing secret use the platform credential store
with the current-user DPAPI fallback on Windows. Settings show the scheme, host
and port. ClawScribe excludes the full URL, signing secret and body from
logs and delivery history.

Microsoft session persistence stores the refresh token and associated account
metadata, not the short-lived access token. It first uses the platform credential
store. On Windows, the file fallback is DPAPI-encrypted for the current user.
Legacy plaintext fallback is removed only after migration to encrypted storage
succeeds; a failed migration can leave that existing file in place. Non-Windows
platforms do not write a new plaintext Microsoft-token fallback.

Treat application settings, database files, authentication directories, and their
backups as sensitive. Do not publish them as diagnostics or attach them to an
issue without review and redaction.

Credential protection does not encrypt meeting recordings,
transcripts, summaries, logs, or the entire database. Local data relies on the
Windows account, filesystem permissions, device/storage protection, and backup
practices.

## iCalendar sources

Importing an `.ics` file saves a local copy, which can contain private
calendar details. Adding a subscription lets ClawScribe retrieve calendar
data from its provider over HTTPS. The subscription URL uses protected
credential storage. Removing the source stops refreshes and removes the
local source from the app.

A linked recording saves the event title, invitees and your attendance review
with the meeting. Library backups can contain those details. An event that
ClawScribe linked automatically stays on your device: its title is not sent to
a summary or knowledge provider, even when it became the meeting title, and
nothing is sent about its invitees. When you review the event and save it, the
title goes to summary and knowledge providers, and summaries receive only the
names of people you checked as present. Email addresses are never sent. Calendar
descriptions, attachments and subscription URLs are excluded.

## Call detection

When enabled, call detection checks local Windows processes, window titles
and call controls for the meeting apps you selected. For supported websites,
it reads the browser's address bar to identify the meeting service and room.
The automation keeps the call identifier in memory and runs on your PC.
Choosing **Join and record** opens the selected invitation in Teams or your
browser and temporarily checks for that call, even with continuous detection
off. If the call cannot be identified as new and matched to the invitation,
ClawScribe requires confirmation or leaves the recording stopped.

## Local MCP clients

The optional local MCP server uses the launching client's standard input and
output. Its access setting starts off and applies to apps running as the
current user. While enabled, clients can read saved meeting titles, notes,
summaries and transcripts, and obtain paths to saved recordings. A client
with access to those files can also read the audio. Turning access off
revokes subsequent reads. A connected client determines how it stores or
transmits content it has already received.

## Application Analytics

ClawScribe contains no application analytics or telemetry client. The backend
analytics module has been removed; remaining frontend analytics calls are inert.
Recording, summary, and notification logs omit meeting names and notification
contents. This does not cover telemetry independently generated by the operating
system or an external provider you choose to use.

## Review, Export, And Deletion

Deleting a meeting removes its database records, including transcripts and their
revisions/corrections, summaries and the previous summary, notes, chat, tags,
bookmarks, and summary context. It also attempts to remove app-managed export
history, Codex scratch runs, generated outputs under app data, and the cached
transcript recovery copy. Cleanup failures are reported; they do not restore the
deleted database record.

**Also delete the recording files** is selected by default. It removes the
recording folder, including audio, transcript copies, metadata, recovery originals,
and generated documents, only when the folder passes the app's ownership checks.
Folders outside the configured/default or restored-recording locations, shared
with another meeting, missing an app recording marker, or reached through links
or junctions are kept and reported. File-access failures are also reported.
Unchecking the option keeps that folder and everything in it, including Codex
outputs saved there. During recording, library deletion is available only with
recording-file deletion turned off.

Pre-migration database snapshots in the app-data `backups` folder can still
contain deleted meetings. Creating a snapshot retains the newest two; startup
cleanup removes snapshots older than 14 days. They may remain longer while the
app is closed or if cleanup fails. Library backup archives are separate,
unencrypted copies and are not removed by meeting deletion. Exports, manual
copies, and data already sent to third parties also require separate handling.
See [local library tools](docs/local-library.md#deleting-meetings).

Follow your meeting's recording and data-sharing requirements. Review generated
notes against the source before sharing them: recognition and model-generated
content can be inaccurate.

## Implementation References

- Microsoft tokens: `frontend/src-tauri/src/exports/token_store.rs`
- Provider key settings: `frontend/src-tauri/src/database/repositories/setting.rs`
- Protected provider credentials: `frontend/src-tauri/src/credentials.rs`
- Confluence credentials: `frontend/src-tauri/src/exports/confluence.rs`
- Codex sign-in and output cleanup: `frontend/src-tauri/src/summary/codex_provider.rs`
- Database snapshots: `frontend/src-tauri/src/database/manager.rs`
- Recording-folder deletion guards: `frontend/src-tauri/src/database/meeting_files.rs`
- [Meeting quality and processing limits](docs/meeting-quality.md)
- [Microsoft Graph integration](docs/integrations/microsoft-graph.md)
- [Windows release verification](docs/windows-release.md)

ClawScribe is distributed under the [MIT License](LICENSE.md). Upstream
attribution is in [NOTICE.md](NOTICE.md) and [UPSTREAM.md](UPSTREAM.md).
