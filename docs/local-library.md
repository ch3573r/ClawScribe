# Local meeting library

## Word documents

Open a saved meeting and select **Export → Word document (.docx)**. Choose the summary, the full
transcript, or both, then save a `.docx` file. Speaker labels and timestamps are
optional. Transcript timestamps use elapsed recording time (including hours),
with the legacy timestamp as a fallback when an audio offset is unavailable.
Export runs locally without Word, Microsoft sign-in, or a network
connection. The summary uses the current editor contents; transcripts use the
complete saved database snapshot rather than the visible page. Basic headings,
bullets, and paragraphs are supported; advanced editor formatting is not retained.

The local main window's Tauri capability grants `dialog:allow-save` for Word
export and backup destinations, and `dialog:allow-open` for restore archives.
These permissions are required in the packaged app even though the dialog plugin
is registered in Rust. The frontend regression suite checks the shipped capability;
browser tests with mocked native calls cannot verify dialog access.

## Shared document export options

The **Export** menu groups local Word, Confluence, OneNote, and OneDrive DOCX/PDF.
Each destination lets you choose **Summary only**, **Transcript only**, or
**Summary and transcript**, with optional speaker labels and transcript timestamps.
Formatting switches are disabled for summary-only exports. The menu remains
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
Meetings page can filter by a project or show only untagged meetings. The filter
also applies to transcript search results. Tags are stored in SQLite and refreshed
across views after changes.

## Bookmarks

Select **Bookmark** during recording to mark the current position as “Review
this.” The timestamp excludes pauses. Markers are stored immediately and linked
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

Version 1 archives have a JSON manifest and explicitly listed recording files.
Only meeting-related database tables and known media/metadata files are included.
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
