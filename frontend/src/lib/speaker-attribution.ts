type SpeakerRow = { speaker?: string | null };

type SpeakerRows = readonly SpeakerRow[] | null | undefined;

export function hasSavedSpeakerLabels(...rowGroups: SpeakerRows[]): boolean {
  return rowGroups.some(rows => rows?.some(row => Boolean(row.speaker?.trim())));
}

/** Saved diarization/manual labels are independent of live source tagging. */
export function shouldShowSpeakerAttribution(
  sourceAttributionEnabled: boolean,
  hasRecordingFolder: boolean,
  ...rowGroups: SpeakerRows[]
): boolean {
  return (sourceAttributionEnabled && hasRecordingFolder) || hasSavedSpeakerLabels(...rowGroups);
}

export function canDetectSpeakers(hasRecordingFolder: boolean, transcriptCount: number): boolean {
  return hasRecordingFolder && transcriptCount > 0;
}
