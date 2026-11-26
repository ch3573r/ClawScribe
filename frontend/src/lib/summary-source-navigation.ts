import type { Transcript } from '@/types';

interface SourcePosition {
  transcript_id: string;
  transcript_index: number;
  timestamp: number | null;
  stale: boolean;
}

export async function summarySourcePlaybackOffset(
  source: SourcePosition,
  readRow: (index: number) => Promise<Pick<Transcript, 'id' | 'audio_start_time'> | undefined>,
): Promise<number | null> {
  let seconds = source.timestamp;
  if (source.stale) {
    // The core withholds the old timestamp for stale evidence. Use only the
    // current offset of the original row, and reject any row now at its index.
    const row = await readRow(source.transcript_index).catch(() => undefined);
    seconds = row?.id === source.transcript_id ? row.audio_start_time ?? null : null;
  }
  return seconds !== null && Number.isFinite(seconds) && seconds >= 0 ? seconds : null;
}
