/**
 * Find in a saved meeting's transcript. Matching is literal, case-insensitive
 * and Unicode aware (so "Über" matches "über"), and never builds a regular
 * expression from user input.
 */

export interface FindableSegment { id: string; text: string }

export const MAX_FIND_QUERY_LENGTH = 200;

function fold(value: string): string {
  return value.toLocaleLowerCase();
}

export function normalizeFindQuery(query: string): string {
  return query.trim().replace(/\s+/g, ' ').slice(0, MAX_FIND_QUERY_LENGTH);
}

export interface FindRange { start: number; end: number }

/** Character ranges of every non-overlapping match of the query in the text. */
export function findRanges(text: string, query: string): FindRange[] {
  const needle = fold(normalizeFindQuery(query));
  if (!needle) return [];
  const haystack = fold(text);
  const found: FindRange[] = [];
  if (haystack.length === text.length) {
    for (let from = haystack.indexOf(needle); from >= 0; from = haystack.indexOf(needle, from + needle.length)) found.push({ start: from, end: from + needle.length });
    return found;
  }
  // Folding changed the length (a few scripts do), so compare character by character.
  const chars = Array.from(text);
  let offset = 0;
  for (let start = 0; start < chars.length;) {
    let folded = '';
    let end = start;
    let length = offset;
    while (end < chars.length && folded.length < needle.length) { folded += fold(chars[end]); length += chars[end].length; end++; }
    if (folded === needle) { found.push({ start: offset, end: length }); offset = length; start = end; }
    else { offset += chars[start].length; start++; }
  }
  return found;
}

export interface TranscriptMatch { id: string; index: number; count: number }

/** Segments containing the query, in transcript order, with their position. */
export function findTranscriptMatches(segments: readonly FindableSegment[], query: string): TranscriptMatch[] {
  if (!normalizeFindQuery(query)) return [];
  const matches: TranscriptMatch[] = [];
  segments.forEach((segment, index) => {
    const count = findRanges(segment.text, query).length;
    if (count > 0) matches.push({ id: segment.id, index, count });
  });
  return matches;
}

/** The match after or before the current one, wrapping at the ends. */
export function stepMatch(current: number, total: number, direction: 1 | -1): number {
  if (total <= 0) return -1;
  if (current < 0 || current >= total) return direction === 1 ? 0 : total - 1;
  return (current + direction + total) % total;
}

export interface HighlightPart { text: string; match: boolean }

/** Split text into plain and matching parts for rendering with <mark>. */
export function splitForHighlight(text: string, query: string): HighlightPart[] {
  const ranges = findRanges(text, query);
  if (!ranges.length) return [{ text, match: false }];
  const parts: HighlightPart[] = [];
  let cursor = 0;
  for (const { start, end } of ranges) {
    if (start > cursor) parts.push({ text: text.slice(cursor, start), match: false });
    parts.push({ text: text.slice(start, end), match: true });
    cursor = end;
  }
  if (cursor < text.length) parts.push({ text: text.slice(cursor), match: false });
  return parts;
}
