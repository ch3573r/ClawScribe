import type { ProjectFilter } from './library';

export type SavedSource = 'summary' | 'note' | 'bookmark';
export interface SavedReference {
  meeting_id: string;
  source: SavedSource;
  id: string;
  start: number;
  end: number;
  fingerprint: string;
}
export interface SearchExcerpt {
  text: string;
  highlights?: [number, number][];
  clipped_start?: boolean;
  clipped_end?: boolean;
}

export interface SavedHit extends SearchExcerpt {
  reference: SavedReference;
  title: string;
  date: string;
  seconds: number | null;
}
export const sourceLabel: Record<SavedSource, string> = { summary: 'Summary', note: 'Saved note', bookmark: 'Bookmark' };

export function searchHighlightParts(text: string, ranges: SearchExcerpt['highlights'] = []) {
  if (!ranges.length) return [{ text, match: false }];
  const source = new globalThis.TextEncoder().encode(text);
  const decoder = new globalThis.TextDecoder('utf-8', { ignoreBOM: true });
  const parts: { text: string; match: boolean }[] = [];
  let cursor = 0;
  for (const [start, end] of ranges) {
    if (start < cursor || end <= start || end > source.length) continue;
    if (start > cursor) parts.push({ text: decoder.decode(source.subarray(cursor, start)), match: false });
    parts.push({ text: decoder.decode(source.subarray(start, end)), match: true });
    cursor = end;
  }
  if (cursor < source.length) parts.push({ text: decoder.decode(source.subarray(cursor)), match: false });
  return parts;
}

export function supportsPersonalExcerpts(provider: string | null | undefined) {
  // Match the endpoint-verifiable providers and aliases in Rust's LLMProvider.
  switch (provider?.toLowerCase()) {
    case 'builtin-ai': case 'ollama': case 'groq': case 'claude': case 'openrouter':
    case 'openai': case 'custom-openai':
    case 'local-llama': case 'localllama': case 'openai-compatible': case 'api-key':
      return true;
    default:
      return false;
  }
}

let passage: { token: string; reference: SavedReference } | null = null;
export function savedPassageUrl(reference: SavedReference, fromArchive = false) {
  const token = globalThis.crypto.randomUUID();
  passage = { token, reference };
  const params = new URLSearchParams({ id: reference.meeting_id, savedSource: token });
  if (fromArchive) params.set('from', 'archive');
  return `/meeting-details?${params}`;
}
export function savedPassage(token: string, meetingId: string) {
  return passage?.token === token && passage.reference.meeting_id === meetingId ? passage.reference : null;
}

export interface MatchScope {
  notes?: SavedReference[];
  question?: string;
  ids: string[];
  project: ProjectFilter;
  from: string;
  to: string;
}
// The draft and explicitly checked locators cross into Meeting memory in memory only.
let pendingScope: MatchScope | null = null;
let onScope: (() => void) | undefined;
export function openMatchScope(scope: MatchScope) {
  if (!scope.ids.length) return;
  pendingScope = { ids: [...new Set(scope.ids)], project: { ...scope.project, tags: [...scope.project.tags] }, from: scope.from, to: scope.to, question: scope.question?.trim() || undefined };
  if (scope.notes?.length) pendingScope.notes = scope.notes
    .filter(note => note.source !== 'summary' && scope.ids.includes(note.meeting_id)).slice(0, 8)
    .map(({ meeting_id, source, id, start, end, fingerprint }) => ({ meeting_id, source, id, start, end, fingerprint }));
  onScope?.();
}
export function receiveMatchScope(receive: (scope: MatchScope) => void) {
  const take = () => {
    const scope = pendingScope;
    pendingScope = null;
    if (scope) receive(scope);
  };
  onScope = take;
  take();
  return () => { if (onScope === take) onScope = undefined; };
}
