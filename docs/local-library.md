# Local meeting library

## Archive review and storage

Open **Meetings → Archive** to see transcript and summary status alongside
titles, dates, and project tags. The archive shows whether audio was recorded or
imported and its measured duration when available, including audio-only recordings.
New recordings use the time capture began, including when Stop or recovery saves
the meeting on another day. A legacy recording without a readable start time
uses its save time. Imports keep the date selected during import.
Older meetings may have no duration or source. **Meetings that need attention** puts saved
meetings with recording warnings, missing transcripts, or failed or cancelled
summary jobs before the rest of the archive. Select **Needs attention** to show
only those meetings. The title, project, date and recurring series filters
apply to both groups and to Archive search. Use the row action to open transcription, review the
recording, retry a recording-folder copy, or return to the summary. Starting transcription or generation
still requires a choice in the existing meeting tools. A missing summary
alone is optional. Recovery files from another computer must be recovered there.

Use **Storage** for measured file sizes of audio and recovery files in
registered saved meeting folders, the library database and indexes, reference
originals, and local database snapshots. Sizes use filesystem metadata and
count each shared recording folder once. Unavailable recording folders and
skipped files are reported. Unattached interrupted recordings, user-created
backup ZIPs, and model downloads are outside these totals.

**Settings → General → Storage** also opens **Library storage** and **Backup and
restore**. Use **Show storage by meeting** for a paginated breakdown of audio,
recovery files, and other files in each meeting's recording folder. Folders
that exceed the file scan limit show a partial measurement. **Choose folder**
changes where future recordings are saved.

To free space, choose **Review saved audio** in a meeting row's menu, or use
**Select meetings → Review selected audio**. Selection applies to the currently
visible rows, up to 100 at a time. The confirmation lists the eligible copies
with their sizes and explains which meetings must keep their audio. Only the
exact reviewed copy can be removed. The app checks again before each removal
and keeps audio when a recording or transcription job is active, the folder
is shared or linked, no transcript is saved, recovery data is present, or
changes to the recording-folder copy are pending. Older recordings, including
older retranscriptions, may lack the information needed to verify ownership.

Removal is permanent and turns off playback and retranscription from that
copy. The meeting, its measured duration and source, transcript, summaries, notes, reference originals,
recording-folder support files, and existing backups remain available.
Imported originals outside the meeting folder remain where they were.
ClawScribe only removes audio after confirmation.

Use **Bulk actions** to add project tags to the selection, export a Markdown
or Word file per meeting, start a Meeting memory conversation with those
meetings selected, or move them to Trash. Export asks where to save each file.
Cancel a save dialog to stop the batch. Recording files stay on disk through
Trash retention. Opening a scoped
Meeting memory conversation leaves the question and sending under your control.

## Open actions and saved live answers

In **Meetings → Open actions**, filter unfinished reviewed actions by owner
text, inclusive due dates, or a reviewed recurring series. Due-date filters
hide actions without a date. **Review action** opens its saved meeting and
focuses its **Done** control. Choose **Save actions** to commit an edit or
completion. Open the meeting's **Export** menu to review selected tasks for
Planner or Microsoft To Do. The action source is available when the reviewed
action was saved with a citation.

During live assistance, choose **Save answer to meeting notes** on a completed
reply. **Saved live answers** in the saved meeting shows the provider, the
finalized-through time, and the original excerpts from the recording. Citations
to other saved meetings or documents display only while the original references
still verify. Use **Show in transcript** to open a uniquely matched passage or
**Play from here** when the cited recording and its offset are available.
Use **Copy** to copy the answer as Markdown or **Insert into summary** to append
it to an existing summary. Source numbers refer to the original saved answer.
Deleting a source meeting or reference document also removes any
saved live answer that used it, including answers without a citation. Detaching
a shared document from a meeting removes answers that used it through that meeting.
Early development builds saved answers without a complete record of their
sources. Deleting any source meeting or document, or detaching any document,
removes all of those older answers. Editing or retranscribing a source keeps
the saved answer, but citations to changed passages may no longer display.
Choose **Remove** to delete a saved answer from the meeting.
Unselected live chat clears at Stop. Saved answers attach to that specific
recording at Stop or during local recovery and can be removed from **Manage
pending notes** if the recording was never saved. Portable archives
include explicitly saved answers whose only sources were finalized excerpts
from that recording.

## Local assistant access

Open **Settings → Assistant access** and enable **Allow
read-only access**. Choose your client format. For Claude Desktop, add the
`clawscribe` entry to `mcpServers` in its JSON configuration. For Codex,
add the displayed table to `config.toml`. Keep other server entries and
reconnect your client. For clients with separate fields, use the
ClawScribe executable as the command and `--mcp-stdio` as the sole argument.
Choose **Test MCP server** to check that the local process responds and lists
the tools for your enabled permissions.
The client starts its own local process, including when the desktop window
is closed. See the [Claude Desktop setup guide](https://modelcontextprotocol.io/docs/develop/connect-local-servers)
and [Codex MCP configuration](https://developers.openai.com/codex/mcp).

| Tool | Result |
| --- | --- |
| `list_meetings` | Saved meeting IDs, titles and dates, newest first. Accepts inclusive `from` and `to` dates, `tags`, `tag_mode` (`any` or `all`), and `untagged`. |
| `list_open_actions` | Open reviewed actions across saved meetings, newest meeting first. Follow `next_offset` for more. |
| `search_meetings` | Meetings matching literal text in their title or current transcript. |
| `search_passages` | Up to 25 current transcript passages containing all search terms, ignoring letter case. Use double quotes for a phrase. Accepts the same date and tag filters as `list_meetings`. Set `semantic` to `true` to request related passages when the local index and model are ready. |
| `get_meeting` | Saved meeting summary, notes, tags, bookmarks and attachment names, formats and sizes. Use `next_metadata_offset` to continue the metadata lists. |
| `get_transcript` | Current transcript text in recording order with available speaker labels and timestamps. |
| `get_audio` | A local path for a verified saved audio file, or null when unavailable. |

Enable **Allow read-only follow-through** for two narrower tools. You can enable
this scope by itself. In a saved meeting, choose **Copy meeting reference** in
the meeting menu and provide that value as `meeting_id` to the client.

| Tool | Result |
| --- | --- |
| `get_meeting_metadata` | Created date, recording duration when known, and title when permitted by the calendar review rules. |
| `list_reviewed_actions` | Open reviewed action IDs, titles, user-entered owners, and due dates for that meeting. Use the returned offset to continue. |

The follow-through scope omits attendee records, calendar identifiers,
transcripts, recordings, notes, citations, and export history.

Use the meeting ID from a list or search result. For passage matches, use
the segment ID when reading the transcript. If passage search reports
`limited`, narrow the query or date and project filters. Follow the returned
offsets for long transcripts and notes. Attachment results contain metadata
only. Calendar invitations and the contents of reference documents stay in
the desktop library.

Each permission grants its scope to local clients running under your user
account. A client may send retrieved content to its own provider. Turning a
switch off blocks delivery for that scope, including from clients already
connected. Copies already read are under the client's control.

## Archive search

Open **Meetings → Archive** to search saved titles, current transcript
passages, visible saved summaries, personal notes and timestamped bookmarks.
All unquoted terms must appear in the same title or text entry, in any order.
Put an exact phrase in double quotes, for example `"offene Punkte" Budget`.
An opening quote without a closing quote is ignored, so the words match as
separate terms.
Search ignores letter case, including German umlauts and capital sharp S, and
treats punctuation such as `%` and `_` literally. Projects, inclusive dates and **Needs attention**
apply to every source. Archive and Meeting memory date filters use the day shown
in your local time zone. Older entries that store only a calendar date keep that
day. Archive results are grouped by meeting. The sidebar shows one preview per
matching source type in a meeting. Highlights mark the matches in each excerpt.

Open a match to see its source. Transcript passages open at their verified
position, including later pages. **Play passage** opens the transcript and
plays from that position when saved audio is ready. Summary and note matches show a verified
excerpt. Saved notes open with their surrounding text, and bookmark matches
open the marker in the meeting, where you can play from its recording time. A changed or
deleted source prompts a new search. **Back to archive** restores your search,
filters and sort order for the current app session.

Queries accept up to 256 characters. The first page shows up to 50 matches
per source type. Choose **Load more** to continue searching saved summaries,
notes and bookmarks. ClawScribe checks the editor's visible text before
applying the result limit. It searches saved entries up to 512 KiB each.
If another search limit leaves some sources unchecked, Archive displays a notice.
Date and project filters search a smaller set. Literal search uses a local substring index to find candidates,
then checks the saved text and its exact offset. Short searches and terms without
an indexable substring use a bounded scan. No download or answer provider is required.
Select **Hybrid** beside Archive search to also find passages with similar
meaning. This option requires a ready local semantic index in **Settings →
Meeting memory**.

**Ask about these results** opens a fresh Meeting memory conversation scoped
to the displayed matching meetings, including when results are limited.
The Archive carries over project and date filters and fills an editable draft
with the search text. Review the selected meetings and draft, then choose **Ask** to use the configured summary
provider. Answers use the existing transcript evidence and eligible saved
summary overviews. Personal notes and bookmarks stay local by default. Check
**Review excerpt with a question** on individual results to bring up to eight
excerpt references into the composer. Opening the composer sends nothing.
**Review and ask** resolves each excerpt locally and shows its exact text, the
question, and the provider, model, endpoint and configured organization or
project. Remove an excerpt or go back to edit the question before confirming.
**Confirm and send** allows one attempt with that selection. Changes to the
sources, question, scope or destination require a fresh review. A failed
attempt also requires another review.

Reviewed requests send the displayed snippets as personal context alongside
the usual meeting evidence. They omit prior conversation. Their saved questions
and answers remain visible locally and are excluded from future provider
history. The built-in model and providers with an identifiable HTTP endpoint
support this review. Codex and Claude Code manage their own connections and
hide personal-excerpt choices. The local assistant's `search_meetings` tool continues
to match only titles and current transcripts.

## Transcript review

The first transcript page can be read, searched and corrected while the full
timeline loads. When the timeline arrives, Find and playback can follow the
whole meeting. If audio is unavailable, the transcript still supports reading,
copying and corrections. Choose **Check again** to retry an attached folder.

Open **Replace…** from Find to use the current search text in the meeting-wide
replacement dialog. **Whole words only** helps replace names that are also
part of longer words. Review the examples and affected passage count before
choosing **Replace all**. Text corrections and Undo update the existing list
in place. A verified citation to an unchanged passage can still open it.

## Meeting memory

Switch to **Meeting memory** for ranked source search and cited questions. Its
searchable picker lets you select meetings and remove selected chips, or check
**Search all saved meetings**. A project or inclusive date filter is enough to
start asking about matching meetings. When meetings are selected, filters
constrain that selection. All-meetings mode searches every meeting eligible under those filters;
turning it off restores your previous meeting selection. Empty constrained meeting selections never broaden into a library
search. You can draft a question before choosing its sources. Leaving Meetings
and returning in the same window restores the selected meetings and filters.
Ranked results show the canonical excerpt, original title/date/speaker,
and actual keyword or semantic + keyword mode. Keyword search stays usable
without enabling or downloading an embedding model.

The first question creates a saved library conversation automatically. The
conversations menu lists them newest first and lets you start, rename or
delete a saved conversation. Opening an existing
conversation restores its last submitted meeting scope, including the project
and date filters. Changes made since the last question are remembered while the
app is open. Select reference files for the current conversation before asking
with them. Use **Copy answer** to copy an assistant response as Markdown.
Questions use the summary provider configured in Settings. That provider may
be local or cloud; retrieval and embedding inference remain local. Answers
show the executed provider/model and retrieval mode. Cancel and scope changes
invalidate pending work; retries reuse the pending request identity. Clearing
a library conversation removes it from the list. Clearing meeting chat removes
its history. Existing saved meeting chat,
including legacy messages, remains readable. In a meeting, chat defaults to
only that meeting. Open **Context** and choose **Other saved meetings** to expand
the scope with a searchable meeting picker and project filters. **Search mode**
is in the chat's **More options** menu. Expanded questions use a separate
saved library conversation: create or choose one in the chat. Turning off the
expansion restores this meeting's own history. The library conversation remains
available when expanding again or through the saved conversation selector.

Citation tags retain their immutable request-local numbering, including grouped
and range syntax. Select a grouped citation to choose its individual source.
Unknown or malformed tags remain plain text. Short reply citations can offer a
separate **Preceding question context** link; this is labeled context supplied
by the backend, not an additional tag emitted by the answer model. Both
references are independently verified before navigation.

**Source passage** separates original labels from current canonical metadata.
Changed, historical, deleted, or invalid sources show a reason and cannot
reveal or play as current evidence. Ask again to retrieve fresh evidence.
Current citations can **Show in transcript**, including passages beyond the
first page, or **Play from here** when a verified offset and saved audio are
available. Unknown offsets never seek to zero. Audio readiness and failure are
visible; a valid transcript can still be reviewed without playable audio.
Citation handoffs are ephemeral: after an application restart, reopen the
source reference instead of reusing a previous navigation URL.

Under **Settings → Meeting memory**, enable the optional local embedding model,
download it, inspect ready/pending/failed index counts, Pause,
Retry/resume, or Rebuild. Foreground recording and inference take priority.
After upgrade, an edit in a different transcript row leaves verified passages navigable, and
the indexer reuses vectors for unchanged passages. Broad library questions
rank matches across the selected scope within the model's input budget.
Index rebuilds do not renumber saved citations. Downloads never start
automatically from a search. The download contains the pinned E5 ONNX model
and tokenizer, about 465 MiB combined. Transfer progress shows bytes and the
current file; integrity verification is a separate stage from semantic indexing.
**Verify installed files** checks existing files and downloads only missing or
invalid ones. It does not repair meeting data. The multilingual model supports
English and German retrieval; its fixed tokenizer, pooling and 384-dimensional
vectors must stay compatible with the saved index. ONNX Runtime runs this model
locally on CPU. The configured summary provider answers questions using the
retrieved passages; the embedding model itself does not write answers.

Automated helper and native resolver checks cover request ownership and
canonical navigation. Installed offline retrieval, provider-failure recovery,
native focus/citation clicks, and audio playback still require manual acceptance
on an isolated profile under the preview policy. They are not established by
source tests or backend resolver tests.

## Reference documents

Use **Meetings → Meeting memory → Attach file** to choose a PDF, DOCX, TXT or
Markdown file. A single selected meeting is used automatically; otherwise pick
the destination meeting in the searchable dialog. The destination respects the
current project/date filters and joins the selected sources after attachment.
With no meetings available, save a meeting or adjust those filters first. An
attached file is selected for the current question once its scope is verified.
You can also use the paperclip beside the compact chat's question field, or
**Context → Add file**. Selected references appear above the question field;
remove a chip to exclude that file from the question. Suggested questions fill
the field without sending. Extraction stays on this PC. PDFs need a text layer; scanned or
encrypted PDFs are unsupported. Limits are 25 MiB per original, 500 PDF pages and
2 MiB of extracted text, with additional archive/XML and extraction-time limits.
A file near the text limit may need splitting because anchor metadata also counts
toward the bounded extraction response.

Preview references locally and select the ones a question may use. Library chat
lists references attached to the meetings in scope. **Allow reference sharing**
above the question enables reference excerpts for non-Built-in AI providers
in the current conversation. This permission is off by default and can be
revoked in the reference controls. Disabling it cancels active answers that use
references. Keyword search and previews work offline without that permission.

Document citation chips open the extracted page or paragraph. They describe
reference material separately from the recorded discussion; they do not seek in
meeting audio. Changed or replaced content is stale and requires a new question.
Detaching a reference removes its extracted text and original file when no other
meeting uses it, and invalidates dependent answers. References shared with another
meeting are kept. Permanent deletion removes the reference from all attached
meetings and invalidates dependent answers.
Indexing status remains visible; retry a failed index from the reference actions.

## Local documents

Open a saved meeting and select **Export → Word document (.docx)**. Choose the summary, the full
transcript, or both, then save a `.docx` file. Speaker labels and timestamps are
optional. Transcript timestamps use elapsed recording time (including hours),
with the legacy timestamp as a fallback when an audio offset is unavailable.
Export runs locally without Word, Microsoft sign-in, or a network
connection. The summary uses the current editor contents; transcripts use the
complete saved database snapshot rather than the visible page. Headings, bullets,
paragraphs, and Markdown tables are supported; advanced editor formatting is not retained.

Choose **Export → Markdown (.md)** to save the same selection as UTF-8 Markdown.
The file also includes timestamped notes and bookmarks, plus saved duration,
confirmed attendee names, and project tags when available.

ClawScribe opens the native save dialog and authorizes the chosen path for that
export. Local document exports require no Microsoft connection.

## Shared document export options

The **Export** menu groups local Word and Markdown, Confluence, OneNote, and OneDrive DOCX/PDF.
Each destination lets you choose **Summary only**, **Transcript only**, or
**Summary and transcript**, with optional speaker labels and timestamps.
**Timestamps** covers source times in the summary and line times in the transcript;
turn it off to export a summary without them. **Speaker labels** applies only to
transcript lines and is hidden for summary-only exports. Tables in the summary are
exported as tables to Word, Markdown, OneDrive DOCX, OneNote, and Confluence. The menu remains
available without a summary so saved transcripts can be exported on their own.

Local Word and Markdown start with both summary and transcript selected. Remote document
destinations start with summary only; select a transcript explicitly before
copying or uploading it. Confluence and OneDrive show the options before export;
OneNote includes them alongside the notebook and section fields. Planner and
Microsoft To Do keep their action-item selection and editing previews.

All document destinations use recording-relative transcript timestamps when
available. Local document exports display the meeting date and time in the local time zone.
Exports use current editor notes and the full saved transcript, independent of
the visible transcript page. A transcript read failure stops the export instead
of silently sending incomplete content. Confluence browser drafts, REST pages,
and clipboard fallback all use the same selected content.

## Project tags

In the archive, open a meeting row’s menu and choose **Edit project tags**.
Use **Project tags** in a saved meeting to choose existing labels from a searchable
list or create new ones. Suggestions show the most-used tags first and preserve
existing spelling. Enter or a comma adds a tag; comma-separated lists can also
be pasted. Remove chips with their buttons or Backspace in the empty search field.
Save up to 20 tags of 60 characters each, or clear every chip to remove all tags. The
Meetings page can filter by one or more projects, matching meetings with any or all
of the selected tags, or show only untagged meetings. The filter also applies to
transcript search results. Tags are stored in SQLite and refreshed
across views after changes.

## Bookmarks and notes

Select the bookmark button (**Bookmark this moment**) in the recording bar to mark
the current position as “Review this.” The tray has the same action. The default
keyboard shortcut is **Ctrl+Shift+F8**, which you can change in Shortcuts settings.
Open **My notes** from the pencil button to write and review timestamped notes
while recording. The timestamp uses the recording time at save and excludes
pauses. In the saved transcript panel, expand **Bookmarks** to rename or remove
markers. While reviewing a saved meeting,
press **B** or choose **Add bookmark** in the transcript menu to add a timestamped
note at the playback position. The dialog opens with the cursor in the note field
(up to 160 characters; an empty note is saved as “Review this”), and Enter saves
it. The time can be changed in seconds. Select a marker to seek in the recording.
Bookmarks also appear as small markers on the playback timeline; each one can be
reached with the keyboard and seeks when chosen.
Playback requires available saved audio. Stop and Interrupted recordings attach
saved markers using the recording's original ID. The ID follows the recording
folder if it moves within the recovery locations. Discarding an interrupted
recording removes its pending markers. Legacy folder-based bookmarks are cleaned
up at startup when their folder is gone and no saved meeting owns it.
If a recording’s folder becomes unavailable, its notes and summary preference
stay available for recovery. In **Settings → General → Storage**, select
**Manage pending notes** to review them. Delete an entry only if you no longer
need those notes, even if you recover the recording later. This action removes
only the selected pending notes and preference.

Bookmarks and their notes are included in backups. The **Use as summary
priorities** switch in **My notes** while recording or under **Bookmarks** in a
saved meeting is off by default. The local library saves the choice for that
meeting and includes it in backups. Enabling it lets
manual and automatic summaries send the first 40 notes in timestamp order to
the summary provider for that run. The switch applies to the whole meeting.

## Backup and restore

Open **Meetings → Backup and restore** to save or restore a ZIP archive. Finish
active recording, import, retranscription, and Microsoft export work before
starting a manual backup or restore. Keep the app open until it completes.

Backups contain saved meetings, transcripts and correction history, notes,
summaries, per-meeting summary context, linked calendar events and attendance,
meeting chat, project tags, bookmarks, reviewed actions, attached reference
originals and extracted blocks, and supported recording files. The archive
includes Microsoft export history from app data, including for meetings without
a recording folder. Restore preserves task and page duplicate protection and
saved OneNote sections in the new profile. Existing meetings keep their own
history. Older archives restore the export history they contain.
If an archived calendar link cannot be read, restore keeps the meeting and
reports how many links it left unset.
Backups also carry custom summary templates and vocabulary. For calendar series
linked to an archived meeting, they carry template and summary language choices,
the setting for including approved notes, and the preferred follow-up
destination. Restore adds missing
custom templates by ID and installs the vocabulary only if the destination has
no vocabulary file. It adds a missing series preference when restoring a meeting
linked to that series. An existing template, vocabulary list, or series
preference takes precedence. An approved note travels only with the archived
meeting whose reviewed event and series still match the note. Restored iCalendar
series still use their original source IDs. Calendar subscriptions, account and
provider approvals, provider settings, credentials, models, UI preferences,
backup schedules, and unfinished recording recovery spools stay with the source
profile. Missing recording folders are reported by meeting title without
blocking the backup. The archive includes their meeting data, and restore
leaves their recording folder unset.
Unreadable folders and folder links still stop backup with a named error.
Nonempty `.audio-spool` and `.checkpoints` directories
remain on the source computer. They do not block backup: available saved audio,
including `audio-recovered.wav`, is included, and the result lists each meeting
whose recovery files were excluded or whose pending recovery has no saved audio.
The warning list stays visible in **Backup and restore** and is stored in the
archive. Choosing a file for another operation clears the previous report;
cancelling the file picker keeps it. Preserve a separate copy of source recording
folders, including hidden files, if you need the recovery originals. Backup never
removes them.
Archives are not encrypted; keep them in trusted storage.

Version 3 backups include explicitly saved live answers attached to an archived
recording when ClawScribe can verify that every source came from that recording.
These answers retain their provider, model, finalized-through time, and original
cited recording excerpts even when audio is unavailable. Answers with incomplete
source records or any dependency on another meeting or reference document are
excluded. The backup reports their count, and restore reports omissions for
meetings it adds. Unsaved live chat, pending answers from unfinished recordings,
connection settings, and sharing approvals stay out of the archive.

Restore adds meetings whose IDs are absent from the library and skips existing
IDs without overwriting them. Restore validates the archive structure and checks
existing IDs before extracting audio, so disk space is needed only for new
meetings' recordings. Audio is staged in a new app-owned directory and database
inserts commit together. If another operation adds a meeting during extraction,
its staged files are removed before commit; cleanup failure rolls back the import.
If a previously skipped meeting is deleted during restore, retry is required so
its audio can be extracted. Invalid archives leave existing meetings alone.
Restore displays saved audio-omission warnings for imported meetings.
Meetings whose recovery files were excluded retain a message directing recovery
to the original computer.
Folder references are rewritten for the destination computer. Restore retains
saved summaries and partial notes that still match their transcripts. Summary jobs
that were still in progress are marked cancelled. Restored jobs do not retry at
startup, even when the same summary destination is approved.

Archives also retain saved knowledge conversations, including independent
library threads and the original citation labels. Existing conversation-owner
IDs are skipped without overwriting their history. A dependent turn is redacted
when an archived source is unavailable or conflicts with a skipped destination
meeting. Restore compares the archived and saved current transcripts even when
the archive stores them as separate streams. A matching source keeps the saved
question, answer, and citation labels. Interrupted answer requests remain
interrupted and require an explicit retry. Restored citations are historical and cannot navigate as current evidence
or enter a future answer prompt, even when a destination revision number happens
to match. New questions establish fresh canonical evidence. Semantic indexes
and model downloads are excluded; restored sources are queued for indexing.

Version 3 archives put transcript segments, chunks, revisions and correction
batches in separate streams of newline-delimited JSON records. Each record is
limited to 128 MiB. The manifest holds the record counts, meeting roster,
remaining metadata, optional follow-through records, and file references. This
lets accumulated transcript history exceed the 128 MiB manifest limit while
export and restore process one record at a time. Archives still have a 100 GiB
total data limit and can contain up to 100,000 recording and reference files.

Version 2 archives add reference metadata, extracted blocks, attachment relations
and original files with verified hashes. Reference sharing permissions are not
included and remain off after restore. Original conflicts are remapped without
overwriting existing files. The same hash/format with differing extracted content
requires restoration into a separate library, preserving existing canonical text.
Versions 1 and 2 remain readable.
Only saved-library database tables and known media/metadata files are included.
The meetings table is required; missing known child tables are treated as empty
so adding optional tables does not invalidate older version-1 backups.
The optional `incomplete_audio` manifest field records omissions by meeting index;
older archives without it remain readable.
Restore rejects unknown tables/columns, unsafe paths, duplicate files, links,
unsupported versions, invalid record counts, and missing or extra archive
entries. The manifest remains limited to 128 MiB. File copying runs on a blocking
worker and streams audio. Transcript records are staged on disk, so the app data
volume needs temporary space for those records during backup and restore.
This is a saved-library archive, not a replacement for crash recovery or a system
backup. Test restoring a backup before relying on it as your only copy.

Invalid, oversized, and unavailable local template files or locations are skipped.
The operation reports the number of skipped template items. Restore preserves
local templates and deletions. An archive that contains an invalid template is rejected.

### Scheduled backups

In **Backup and restore**, choose a local folder to enable daily backups. The
first backup runs when the app is open and the library is idle. Later backups
run at least 24 hours after the last successful one. ClawScribe checks once a
minute and defers while recording, transcription, Built-in AI notes, an export,
or an archive operation is active. Starting a recording interrupts a scheduled backup that
is still reading the library. ClawScribe discards the partial ZIP and retries
when the library is idle. A completed ZIP can finish saving during recording.
If ClawScribe can measure free space, it budgets the ZIP and temporary
transcript records to leave room for recording. The dialog shows the current
destination, last successful backup, incomplete audio, skipped template and
excluded saved answer counts, and any recent error.

ClawScribe creates a dedicated folder in the location you choose. After a new
backup is saved, retention keeps the three newest tracked archives. It removes
an older ZIP only when its file identity still matches the saved copy. Manual
copies, unrelated or replaced files, and archives in earlier destinations
remain on disk. Turning off daily backups leaves your archives available.
After an interruption such as a crash, ClawScribe cleans up the recorded
partial ZIP when its identity still matches.

### If the library fails to open

At the startup error dialog, choose **Open recovery library** to open a separate
app data folder. ClawScribe restarts and shows the active recovery library.
Open **Meetings → Backup and restore** to import a ZIP.
The original database, WAL, and recordings remain in their original locations.
The recovery profile has its own recording and reference folders, settings,
and sign-ins.

The recovery choice persists between restarts. After repairing the original
library, choose **Return to original library**, then quit and reopen ClawScribe.
The recovery workspace stays on disk. If the original library still fails to
open, **Open recovery library** reopens the previous workspace when available.

## Moving meetings to Trash

Choose **Move to Trash** in an archive row’s menu, in the bulk actions, or
from the sidebar. The meeting disappears from Archive, search, export, Meeting
memory, and the local assistant. Its transcript, notes, and recording files
stay on disk during retention. Open **Meetings → Trash** to restore it or
delete it permanently. ClawScribe checks for expired meetings at startup and
while running and permanently deletes them after 30 days.

Portable backups include meetings in Trash and any recording files included
by the backup settings. Restoring a copy from backup puts it back in Trash
with a fresh 30-day window. An existing local meeting keeps its current state.

Permanent deletion removes the meeting's database records, including transcript revisions,
previous summaries, context, notes, chat, tags, and bookmarks. App-managed export
history, Codex scratch runs, outputs under app data, and cached transcript recovery
copies are also removed where possible.
References used only by this meeting are deleted, including extracted text and
original files, even when recording files are kept. References shared with another
meeting remain available there. Answers that depended on a deleted reference are
invalidated and their saved content is cleared. Startup also cleans abandoned references from earlier installs
when recording and imports are idle; unknown reference files are kept for at least
24 hours before cleanup. Portable backups include only attached references.

The confirmation when moving a meeting to Trash lets you choose whether to
remove its recording files at permanent deletion. That removes the whole
verified recording folder, including audio, transcript copies, metadata, recovery originals,
and generated documents. The folder must be inside the configured save location,
a supported default recording location, or app-data `restored-recordings`; it must
contain a matching recording marker, have no links or junctions in its path, and not
overlap another meeting's folder. Folders failing these checks or file deletion
are kept. Uncheck the option to keep the folder and its contents.
Permanent deletion waits for recording, transcription, export, or backup work to finish.

If file cleanup could not finish, open **Meetings → Archive → Storage**. Under
**Pending file cleanup**, review the item and the reason it was kept. Close apps
using the files or reconnect an unavailable recording location before retrying.
ClawScribe verifies ownership before moving files to a temporary location beside
the original, then removes the moved files. If removal stops partway through,
the cleanup list shows the temporary location for the next retry. An unavailable
drive, share, or parent folder keeps staged cleanup pending until the location can
be verified again. ClawScribe can clear an empty staging folder when the original
is verified absent, or retry the move after checking it still belongs to the
cleanup entry. For recordings, this requires a matching marker. Review other
unfinished staging locations before removing files manually. A new meeting
using the original path can keep using it. A recording-folder retry before the
files have moved requires a marker that identifies that meeting. If an older
folder has no marker or its marker has no meeting ID, use **Keep files** and review
the folder before removing it manually. **Keep files** clears the item from the
cleanup list and preserves the files at the displayed location. Pending entries
are stored on this device and survive app restarts.

On Windows, a OneDrive Cloud Files placeholder can be part of a configured
recording location. Opening or recovering its contents may require OneDrive to
download them. When you pick a recording location through a link or junction,
ClawScribe saves its verified target and checks the new recording folder before
capture. For an older setting that still points through a link, choose the actual
folder again in **Settings → Recording**. This also makes earlier interrupted
recordings in that target available to the disk scan. Folder deletion continues
to require an unlinked path and verified ownership.

If a recovery scan cannot verify a draft's folder or read its recording marker,
**Interrupted recordings** shows **Folder unverified** and lets you preview the
cached transcript. Reconnect the location or let OneDrive finish downloading,
then choose **Retry scan** in the recovery dialog or under **Settings → General**.
For a draft with a stable recording ID, **Save transcript to library** saves the
full cached transcript. When the audio returns, retry the scan and recover it
into the same meeting. For any unverified draft, **Save transcript copy** saves a local
text file and keeps the draft linked to its original folder. Both options keep
the original recovery data. A scan that confirms the original folder is absent
or belongs to a different recording can offer transcript-only recovery.
ClawScribe checks recording identity and library deletion history before
offering recovery.

Pre-migration database snapshots in app-data `backups` can contain deleted
meetings. Each new snapshot keeps the newest two, and startup cleanup removes
snapshots older than 14 days. Retention can last longer while the app is closed
or cleanup fails. Portable backups are separate, unencrypted archives. Meeting
deletion does not remove these archives or copies exported elsewhere.
