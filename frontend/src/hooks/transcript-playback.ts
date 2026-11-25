import type { TranscriptSegmentData } from '@/types';

interface PlaybackRange {
  id: string;
  start: number;
  end: number;
}

export function indexPlaybackRanges(segments: TranscriptSegmentData[]): PlaybackRange[] {
  const sorted = segments.filter(row => Number.isFinite(row.timestamp))
    .sort((left, right) => left.timestamp - right.timestamp);
  const ranges: PlaybackRange[] = [];
  let coveredUntil = -Infinity;
  for (const { id, timestamp: start, endTime } of sorted) {
    const end = endTime ?? start + 0.75;
    // A fully covered row can never be the first matching row. Keep only
    // increasing end times so overlapping speakers can be searched by end.
    if (end > start && end > coveredUntil) {
      ranges.push({ id, start, end });
      coveredUntil = end;
    }
  }
  return ranges;
}

export function activePlaybackSegment(ranges: PlaybackRange[], time: number | undefined): string | null {
  if (time === undefined || !Number.isFinite(time)) return null;
  let low = 0;
  let high = ranges.length;
  while (low < high) {
    const mid = low + Math.floor((high - low) / 2);
    if (ranges[mid].end <= time) low = mid + 1;
    else high = mid;
  }
  const range = ranges[low];
  return range && range.start <= time ? range.id : null;
}
