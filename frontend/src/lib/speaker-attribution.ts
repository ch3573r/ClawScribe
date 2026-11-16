type SpeakerRow = { speaker?: string | null };

/** Saved diarization/manual labels are independent of live source tagging. */
export function shouldShowSpeakerAttribution(
  sourceAttributionEnabled: boolean,
  hasRecordingFolder: boolean,
  ...rowGroups: (readonly SpeakerRow[] | null | undefined)[]
): boolean {
  return (sourceAttributionEnabled && hasRecordingFolder) ||
    rowGroups.some(rows => rows?.some(row => Boolean(row.speaker?.trim())));
}

export function canDetectSpeakers(hasRecordingFolder: boolean, transcriptCount: number): boolean {
  return hasRecordingFolder && transcriptCount > 0;
}
