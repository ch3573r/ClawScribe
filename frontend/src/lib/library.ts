export interface MeetingTag { meeting_id: string; tag: string }
export interface MeetingBookmark { id: string; seconds: number; label: string }

export interface LibraryArchiveReport {
  meetings: number;
  skipped: number;
  files: number;
  incomplete_meetings?: {
    meeting_id: string;
    title: string;
    recovery_files_excluded: boolean;
    audio_unavailable: boolean;
    recording_folder_missing: boolean;
  }[];
}

export function parseProjectTags(text: string): string[] {
  return [...new Map(text.split(',').map(tag => tag.trim().replace(/\s+/g, ' ')).filter(Boolean).map(tag => [tag.toLowerCase(), tag])).values()];
}

export function suggestTags(allTags: MeetingTag[], selected: string[], query: string): string[] {
  const catalog = new Map<string, { tag: string; meetings: Set<string> }>();
  for (const { tag, meeting_id } of allTags) {
    const key = tag.toLowerCase();
    const entry = catalog.get(key) ?? { tag, meetings: new Set<string>() };
    entry.meetings.add(meeting_id);
    catalog.set(key, entry);
  }
  const excluded = new Set(selected.map(tag => tag.toLowerCase()));
  const search = query.trim().replace(/\s+/g, ' ').toLowerCase();
  return [...catalog.entries()]
    .filter(([key]) => !excluded.has(key) && key.includes(search))
    .map(([, entry]) => entry)
    .sort((a, b) => b.meetings.size - a.meetings.size || a.tag.localeCompare(b.tag))
    .map(entry => entry.tag);
}

export function addTag(selected: string[], raw: string, allTags: MeetingTag[]): string[] {
  const result = [...selected];
  for (const tag of parseProjectTags(raw)) {
    if (Array.from(tag).length > 60) throw new Error('Tags must be 60 characters or fewer.');
    const key = tag.toLowerCase();
    if (result.some(existing => existing.toLowerCase() === key)) continue;
    if (result.length >= 20) throw new Error('Maximum 20 tags');
    result.push(allTags.find(existing => existing.tag.toLowerCase() === key)?.tag ?? tag);
  }
  return result;
}

export function removeLastTag(selected: string[]): string[] {
  return selected.slice(0, -1);
}

export type ProjectFilter = { tags: string[]; untagged: boolean; mode: 'any' | 'all' };

export function matchesProjectFilter(meetingId: string, filter: ProjectFilter, tags: MeetingTag[]): boolean {
  if (!filter.tags.length && !filter.untagged) return true;
  const assigned = new Set(tags.filter(tag => tag.meeting_id === meetingId).map(tag => tag.tag.toLowerCase()));
  if (filter.untagged) return assigned.size === 0;
  const matches = (tag: string) => assigned.has(tag.toLowerCase());
  return filter.mode === 'all' ? filter.tags.every(matches) : filter.tags.some(matches);
}

export function bookmarkTime(seconds: number): string {
  const rounded = Math.max(0, Math.floor(seconds));
  return `${Math.floor(rounded / 3600).toString().padStart(2, '0')}:${Math.floor(rounded / 60 % 60).toString().padStart(2, '0')}:${(rounded % 60).toString().padStart(2, '0')}`;
}

export function safeDocumentName(title: string): string {
  const stem = title.replace(/[<>:"/\\|?*\x00-\x1f]/g, ' ').replace(/\s+/g, ' ').trim().replace(/[. ]+$/, '').slice(0, 120);
  return `Meeting - ${stem || 'notes'}.docx`;
}
