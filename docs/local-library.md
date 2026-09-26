# Local meeting library

## Word documents

Open a saved meeting and select **Export Word**. Choose the summary, the full
transcript, or both, then save a `.docx` file. Speaker labels and timestamps are
optional. Export runs locally without Word, Microsoft sign-in, or a network
connection. The summary uses the current editor contents; transcripts use the
complete saved database snapshot rather than the visible page. Basic headings,
bullets, and paragraphs are supported; advanced editor formatting is not retained.

The local main window's Tauri capability grants `dialog:allow-save` for Word
export and backup destinations, and `dialog:allow-open` for restore archives.
These permissions are required in the packaged app even though the dialog plugin
is registered in Rust. The frontend regression suite checks the shipped capability;
browser tests with mocked native calls cannot verify dialog access.

## Project tags

Use **Project tags** in a saved meeting to assign comma-separated labels. The
Meetings page can filter by a project or show only untagged meetings. The filter
also applies to transcript search results. Tags are stored in SQLite and refreshed
across views after changes. A meeting can have up to 20 tags of 60 characters each.

## Bookmarks

Select **Bookmark** during recording to mark the current position as “Review
this.” The timestamp excludes pauses. Markers are stored immediately and linked
to the saved meeting by its recording folder. In the saved transcript panel,
expand **Bookmarks** to rename or remove markers, add a labeled time in seconds,
or use the current playback time. Select a marker to seek in the recording.
Playback requires available saved audio. An interrupted recording's markers remain
local and become visible when that recording is recovered into the same folder.

## Backup and restore

Open **Meetings → Backup and restore** to save or restore a ZIP archive. Finish
active recording, import, and retranscription work first. The native job permit
prevents these operations from overlapping an archive operation. Keep the app open
until it completes.

Backups contain saved meetings, transcripts and correction history, notes,
summaries, meeting chat, project tags, bookmarks, and supported recording files.
Microsoft export history (`exports.json`) is retained with the recording folder.
Provider settings, credentials, model downloads, UI preferences, browser-stored
meeting context, and unfinished recording recovery spools are excluded. Missing
recording folders cause backup to fail instead of silently omitting their contents.
Archives are not encrypted; keep them in trusted storage.

Restore adds meetings whose IDs are absent from the library and skips existing
IDs without overwriting them. Audio is staged in a new app-owned directory and
database inserts commit together. Invalid archives leave existing meetings alone.
Folder references are rewritten for the destination computer. Interrupted summary
jobs are marked cancelled; their saved output is retained.

Version 1 archives have a JSON manifest and explicitly listed recording files.
Only meeting-related database tables and known media/metadata files are included.
Restore rejects unknown tables/columns, unsafe paths, duplicate files, links,
unsupported versions, and missing or extra archive entries. Limits are 128 MiB of
manifest data, 100 GiB of file data, and 100,000 recording files. File copying runs
on a blocking worker and streams audio instead of loading recordings into RAM.
This is a saved-library archive, not a replacement for crash recovery or a system
backup. Test restoring a backup before relying on it as your only copy.
