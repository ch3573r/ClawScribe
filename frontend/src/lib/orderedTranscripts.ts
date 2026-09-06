import type { Transcript } from '@/types';

export function compareTranscripts(a: Transcript, b: Transcript): number {
  const timeA = a.chunk_start_time ?? a.audio_start_time ?? 0;
  const timeB = b.chunk_start_time ?? b.audio_start_time ?? 0;
  return timeA - timeB || (a.sequence_id ?? 0) - (b.sequence_id ?? 0);
}

const sameTranscript = (a: Transcript, b: Transcript): boolean =>
  a.text === b.text && a.timestamp === b.timestamp && a.speaker === b.speaker &&
  a.is_partial === b.is_partial && a.confidence === b.confidence &&
  a.chunk_start_time === b.chunk_start_time && a.audio_start_time === b.audio_start_time &&
  a.audio_end_time === b.audio_end_time && a.duration === b.duration &&
  (a.word_timestamps === b.word_timestamps || JSON.stringify(a.word_timestamps) === JSON.stringify(b.word_timestamps));

/** Event-owned index. Call outside React state updaters. Snapshots are never
 * mutated; keep the common ordered append O(batch size) plus one array copy.
 * Late corrections are less frequent and use a full rebuild/sort.
 */
export class OrderedTranscripts {
  private byId = new Map<string, Transcript>();
  private snapshot: Transcript[] = [];
  getSnapshot = (): Transcript[] => this.snapshot;
  private key(item: Transcript): string {
    return item.sequence_id === undefined ? `id:${item.id}` : `sequence:${item.sequence_id}`;
  }
  clear(): Transcript[] {
    this.byId.clear();
    this.snapshot = [];
    return this.snapshot;
  }
  merge(incoming: readonly Transcript[], preserveExisting = false): Transcript[] {
    const additions: Transcript[] = [];
    let replaced = false;
    for (const item of incoming) {
      const key = this.key(item);
      const previous = this.byId.get(key);
      if (previous) {
        // A late partial must never overwrite a final recognition result.
        if (preserveExisting || (!previous.is_partial && item.is_partial) || sameTranscript(previous, item)) continue;
        this.byId.set(key, { ...item, id: previous.id });
        replaced = true;
      } else {
        this.byId.set(key, item);
        additions.push(item);
      }
    }
    if (!replaced && additions.length === 0) return this.snapshot;
    if (replaced) {
      this.snapshot = [...this.byId.values()].sort(compareTranscripts);
    } else {
      additions.sort(compareTranscripts);
      const previousLast = this.snapshot[this.snapshot.length - 1];
      this.snapshot = !previousLast || compareTranscripts(previousLast, additions[0]) <= 0
        ? [...this.snapshot, ...additions]
        : [...this.snapshot, ...additions].sort(compareTranscripts);
    }
    return this.snapshot;
  }
}
