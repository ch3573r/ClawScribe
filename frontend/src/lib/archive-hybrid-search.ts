import type { TranscriptHit } from "@/hooks/useLibrarySearch";
import type { Passage } from "@/types/knowledge";

/** Keep Archive's exact matches and add ranked passages for transcripts it did not find. */
export function mergeArchivePassages(literal: readonly TranscriptHit[], ranked: readonly Passage[]): TranscriptHit[] {
  const result = [...literal];
  if (!ranked.length) return result;
  const seen = new Map<string, Set<string>>();
  const record = (hit: TranscriptHit) => {
    const ids = seen.get(hit.meeting_id) ?? new Set<string>();
    ids.add(hit.transcript_id);
    if (hit.evidence.locator.kind === "transcript") hit.evidence.locator.transcript_ids.forEach(id => ids.add(id));
    seen.set(hit.meeting_id, ids);
  };
  result.forEach(record);
  for (const hit of ranked) {
    const locator = hit.evidence.locator;
    if (locator.kind !== "transcript" || locator.meeting_id !== hit.meeting_id || !locator.transcript_ids.length
      || locator.transcript_ids.some(id => seen.get(hit.meeting_id)?.has(id))) continue;
    const passage = { ...hit, transcript_id: locator.transcript_ids[0], start_seconds: locator.start_seconds };
    result.push(passage);
    record(passage);
  }
  return result;
}
