import { invoke } from "@tauri-apps/api/core";
import { archiveDateKey } from "@/lib/archive-review";
import { safeDocumentName } from "@/lib/library";
import { isEditorSummary, readMeetingSummary } from "@/lib/meeting-summary";
import type { SummaryDataResponse } from "@/types";
import type { ArchiveMeeting } from "./ArchiveAudioRemoval";

export type LocalExportFormat = "markdown" | "word";

export async function exportArchiveMeeting(meeting: ArchiveMeeting, format: LocalExportFormat): Promise<boolean> {
  const summary = await invoke<{ data: SummaryDataResponse | null }>("api_get_summary", { meetingId: meeting.id });
  const { summary: notes } = readMeetingSummary(summary.data);
  let markdown = "";
  if (notes && isEditorSummary(notes)) {
    markdown = notes.markdown ?? "";
    if (!markdown && notes.summary_json?.length) {
      const { BlockNoteEditor } = await import("@blocknote/core");
      markdown = await BlockNoteEditor.create().blocksToMarkdownLossy(notes.summary_json);
    }
  } else if (notes) {
    markdown = Object.values(notes).map(section =>
      [`## ${section.title}`, ...section.blocks.map(block => block.content)].join("\n\n"),
    ).join("\n\n");
  }
  const isMarkdown = format === "markdown";
  const path = await invoke<string | null>(isMarkdown ? "pick_markdown_save_path" : "pick_word_save_path", {
    suggestedName: safeDocumentName(meeting.title, isMarkdown ? "md" : "docx", archiveDateKey(meeting.created_at)),
  });
  if (!path) return false;
  await invoke(isMarkdown ? "export_local_markdown" : "export_local_word", {
    meetingId: meeting.id, title: meeting.title, markdown, path,
    includeTranscript: true, includeSpeakers: true, includeTimestamps: true,
  });
  return true;
}
