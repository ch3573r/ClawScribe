import type { Transcript } from "@/types";

export function formatTranscriptTime(
  seconds: number | undefined,
  fallbackTimestamp: string,
): string {
  if (seconds === undefined) return fallbackTimestamp;
  const totalSecs = Math.floor(seconds);
  const mins = Math.floor(totalSecs / 60);
  const secs = totalSecs % 60;
  return `[${mins.toString().padStart(2, "0")}:${secs
    .toString()
    .padStart(2, "0")}]`;
}

export function formatTranscriptLine(
  transcript: Transcript,
  { includeSpeaker = true }: { includeSpeaker?: boolean } = {},
): string {
  const timestamp = formatTranscriptTime(
    transcript.audio_start_time,
    transcript.timestamp,
  );
  const speaker = transcript.speaker?.trim();
  const speakerPrefix = includeSpeaker && speaker ? `${speaker}: ` : "";
  return `${timestamp} ${speakerPrefix}${transcript.text}`;
}
