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
export interface SavedHit {
  reference: SavedReference;
  title: string;
  date: string;
  text: string;
  seconds: number | null;
}
export const sourceLabel: Record<SavedSource, string> = { summary: 'Summary', note: 'Saved note', bookmark: 'Bookmark' };

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
  ids: string[];
  project: ProjectFilter;
  from: string;
  to: string;
}
// Only meeting IDs and archive filters cross into Meeting memory.
// Keep the search phrase and excerpts out of draft questions and route URLs.
let pendingScope: MatchScope | null = null;
let onScope: (() => void) | undefined;
export function openMatchScope(scope: MatchScope) {
  if (!scope.ids.length) return;
  pendingScope = { ids: [...new Set(scope.ids)], project: { ...scope.project, tags: [...scope.project.tags] }, from: scope.from, to: scope.to };
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
