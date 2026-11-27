import { invoke } from "@tauri-apps/api/core";
import { archiveDateKey } from "@/lib/archive-review";
import { safeDocumentName } from "@/lib/library";

export const subtitleFormats = [
  { format: "srt", label: "SRT subtitles (.srt)" },
  { format: "vtt", label: "WebVTT subtitles (.vtt)" },
] as const;

export type SubtitleFormat = typeof subtitleFormats[number]["format"];

export function isSubtitleFormat(format: string | null): format is SubtitleFormat {
  return format === "srt" || format === "vtt";
}

export async function exportMeetingSubtitles(
  meetingId: string,
  title: string,
  createdAt: string | undefined,
  format: SubtitleFormat,
  includeSpeakers: boolean,
): Promise<{ path: string; canOpen: boolean } | null> {
  const path = await invoke<string | null>("pick_subtitle_save_path", {
    format,
    suggestedName: safeDocumentName(title, format, archiveDateKey(createdAt)),
  });
  if (!path) return null;
  const canOpen = await invoke<boolean>("export_local_subtitles", { meetingId, format, includeSpeakers, path });
  return { path, canOpen };
}
