import type { Transcript } from '@/types';

function compare(left: Transcript, right: Transcript) {
  return ((left.chunk_start_time || 0) - (right.chunk_start_time || 0)) ||
    ((left.sequence_id || 0) - (right.sequence_id || 0));
}

/** Insert a unique batch into the existing chronological transcript. */
export function insertLiveTranscripts(current: Transcript[], incoming: Transcript[]): Transcript[] {
  if (!incoming.length) return current;
  if (incoming.length === 1) {
    const row = incoming[0];
    if (!current.length || compare(current[current.length - 1], row) <= 0) {
      return [...current, row];
    }
    let low = 0;
    let high = current.length;
    while (low < high) {
      const mid = low + Math.floor((high - low) / 2);
      if (compare(current[mid], row) <= 0) low = mid + 1;
      else high = mid;
    }
    const result = current.slice();
    result.splice(low, 0, row);
    return result;
  }

  const sorted = incoming.slice().sort(compare);
  if (!current.length || compare(current[current.length - 1], sorted[0]) <= 0) {
    return [...current, ...sorted];
  }
  const result: Transcript[] = [];
  let existing = 0;
  for (const row of sorted) {
    while (existing < current.length && compare(current[existing], row) <= 0) {
      result.push(current[existing++]);
    }
    result.push(row);
  }
  return result.concat(current.slice(existing));
}
