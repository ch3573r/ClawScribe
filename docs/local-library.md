# Local meeting library

## Meeting memory

On **Meetings**, ordinary title filtering remains separate from **Meeting
memory** transcript retrieval. Select meetings explicitly, or check **Search
all saved meetings**. The shared Project filter applies Any/All/Untagged
semantics to title browsing and retrieval; **From** and **To** dates are
inclusive. All-meetings mode searches every meeting eligible under those filters;
turning it off restores your previous meeting selection. Empty constrained meeting selections never broaden into a library
search. Ranked results show the canonical excerpt, original title/date/speaker,
and actual keyword or semantic + keyword mode. Keyword search stays usable
without enabling or downloading an embedding model.

Create or choose a saved library conversation under **Ask about these meetings**.
Questions use the summary provider configured in Settings. That provider may
be local or cloud; retrieval and embedding inference remain local. Answers
show the executed provider/model and retrieval mode. Cancel and scope changes
invalidate pending work; retries reuse the pending request identity. Clear
removes the selected conversation's history. Existing saved meeting chat,
including legacy messages, remains readable. In a meeting, chat defaults to
only that meeting; **Include other saved meetings** is an explicit expansion
with independent meeting and project controls. Expanded questions use a separate
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
download or repair it, inspect ready/pending/failed index counts, Pause,
Retry/resume, or Rebuild. Foreground recording and inference take priority.
Index rebuilds do not renumber saved citations. Downloads never start
automatically from a search. Document and live-assistance controls are outside
this preview.

Automated helper and native resolver checks cover request ownership and
canonical navigation. Installed offline retrieval, provider-failure recovery,
native focus/citation clicks, and audio playback still require manual acceptance
on an isolated profile under the preview policy. They are not established by
source tests or backend resolver tests.

## Word documents

Open a saved meeting and select **Export → Word document (.docx)**. Choose the summary, the full
transcript, or both, then save a `.docx` file. Speaker labels and timestamps are
optional. Transcript timestamps use elapsed recording time (including hours),
with the legacy timestamp as a fallback when an audio offset is unavailable.
Export runs locally without Word, Microsoft sign-in, or a network
connection. The summary uses the current editor contents; transcripts use the
complete saved database snapshot rather than the visible page. Headings, bullets,
paragraphs, and Markdown tables are supported; advanced editor formatting is not retained.

The local main window's Tauri capability grants `dialog:allow-save` for Word
export and backup destinations, and `dialog:allow-open` for restore archives.
These permissions are required in the packaged app even though the dialog plugin
is registered in Rust. The frontend regression suite checks the shipped capability;
browser tests with mocked native calls cannot verify dialog access.

## Shared document export options

The **Export** menu groups local Word, Confluence, OneNote, and OneDrive DOCX/PDF.
Each destination lets you choose **Summary only**, **Transcript only**, or
**Summary and transcript**, with optional speaker labels and timestamps.
**Timestamps** covers source times in the summary and line times in the transcript;
turn it off to export a summary without them. **Speaker labels** applies only to
transcript lines and is hidden for summary-only exports. Tables in the summary are
exported as tables to Word, OneDrive DOCX, OneNote, and Confluence. The menu remains
available without a summary so saved transcripts can be exported on their own.

Local Word starts with both summary and transcript selected. Remote document
destinations start with summary only; select a transcript explicitly before
copying or uploading it. Confluence and OneDrive show the options before export;
OneNote includes them alongside the notebook and section fields. Planner and
Microsoft To Do keep their action-item selection and editing previews.

All document destinations use recording-relative transcript timestamps when
available. The Word document date uses local date and time.
Exports use current editor notes and the full saved transcript, independent of
the visible transcript page. A transcript read failure stops the export instead
of silently sending incomplete content. Confluence browser drafts, REST pages,
and clipboard fallback all use the same selected content.

## Project tags

Use **Project tags** in a saved meeting to choose existing labels from a searchable
list or create new ones. Suggestions show the most-used tags first and preserve
existing spelling. Enter or a comma adds a tag; comma-separated lists can also
be pasted. Remove chips with their buttons or Backspace in the empty search field.
Save up to 20 tags of 60 characters each, or clear every chip to remove all tags. The
Meetings page can filter by one or more projects, matching meetings with any or all
of the selected tags, or show only untagged meetings. The filter also applies to
transcript search results. Tags are stored in SQLite and refreshed
across views after changes.

## Bookmarks

Select the bookmark button (**Bookmark this moment**) in the recording bar to mark
the current position as “Review this.” The timestamp excludes pauses. Markers are stored immediately and linked
to the saved meeting by its recording folder. In the saved transcript panel,
expand **Bookmarks** to rename or remove markers, add a labeled time in seconds,
or use the current playback time. Select a marker to seek in the recording.
Playback requires available saved audio. An interrupted recording's markers remain
local and become visible when that recording is recovered into the same folder.
Startup removes abandoned, unattached recording bookmarks only when their folder
is gone and no saved meeting uses it. Existing or inaccessible recovery folders
are preserved, and cleanup skips active recording.

## Backup and restore

Open **Meetings → Backup and restore** to save or restore a ZIP archive. Finish
active recording, import, and retranscription work first. The native job permit
prevents these operations from overlapping an archive operation. Keep the app open
until it completes.

Backups contain saved meetings, transcripts and correction history, notes,
summaries, per-meeting summary context, meeting chat, project tags, bookmarks,
and supported recording files.
Microsoft export history (`exports.json`) is retained with the recording folder.
Provider settings, credentials, model downloads, UI preferences, and unfinished
recording recovery spools are excluded. Missing
recording folders are reported by meeting title without blocking the backup;
their meeting data is included, and restore leaves their recording folder unset.
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
Folder references are rewritten for the destination computer. Interrupted summary
jobs are marked cancelled; their saved output is retained.

Archives also retain saved knowledge conversations, including independent
library threads and the original citation labels. Existing conversation-owner
IDs are skipped without overwriting their history. A dependent turn is redacted
when an archived source is unavailable or conflicts with a skipped destination
meeting. Interrupted answer requests remain interrupted and require an explicit
retry. Restored citations are historical and cannot navigate as current evidence
or enter a future answer prompt, even when a destination revision number happens
to match. New questions establish fresh canonical evidence. Semantic indexes
and model downloads are excluded; restored sources are queued for indexing.

Version 1 archives have a JSON manifest and explicitly listed recording files.
Only saved-library database tables and known media/metadata files are included.
The meetings table is required; missing known child tables are treated as empty
so adding optional tables does not invalidate older version-1 backups.
The optional `incomplete_audio` manifest field records omissions by meeting index;
older archives without it remain readable.
Restore rejects unknown tables/columns, unsafe paths, duplicate files, links,
unsupported versions, and missing or extra archive entries. Limits are 128 MiB of
manifest data, 100 GiB of file data, and 100,000 recording files. File copying runs
on a blocking worker and streams audio instead of loading recordings into RAM.
This is a saved-library archive, not a replacement for crash recovery or a system
backup. Test restoring a backup before relying on it as your only copy.

## Deleting meetings

Deletion removes the meeting's database records, including transcript revisions,
previous summaries, context, notes, chat, tags, and bookmarks. App-managed export
history, Codex scratch runs, outputs under app data, and cached transcript recovery
copies are also removed where possible; cleanup failures are reported separately.

**Also delete the recording files** is on by default. It removes the whole
recording folder, including audio, transcript copies, metadata, recovery originals,
and generated documents. The folder must be inside the configured save location,
a supported default recording location, or app-data `restored-recordings`; it must
contain an app recording marker, have no links or junctions in its path, and not
overlap another meeting's folder. Folders failing these checks or file deletion
are kept and reported after the database deletion. Uncheck the option to keep
the folder and its contents. While recording or transcription is running, only
deletion with this option off is allowed.

Pre-migration database snapshots in app-data `backups` can contain deleted
meetings. Each new snapshot keeps the newest two, and startup cleanup removes
snapshots older than 14 days. Retention can last longer while the app is closed
or cleanup fails. Portable backups are separate, unencrypted archives. Meeting
deletion does not remove these archives or copies exported elsewhere.
