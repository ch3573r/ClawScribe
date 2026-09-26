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

export function matchesProjectTag(meetingId: string, filter: string, tags: MeetingTag[]): boolean {
  if (!filter) return true;
  const assigned = tags.filter(tag => tag.meeting_id === meetingId);
  return filter === '__untagged' ? assigned.length === 0 : assigned.some(tag => `tag:${tag.tag.toLowerCase()}` === filter.toLowerCase());
}

export function bookmarkTime(seconds: number): string {
  const rounded = Math.max(0, Math.floor(seconds));
  return `${Math.floor(rounded / 3600).toString().padStart(2, '0')}:${Math.floor(rounded / 60 % 60).toString().padStart(2, '0')}:${(rounded % 60).toString().padStart(2, '0')}`;
}

export function safeDocumentName(title: string): string {
  const stem = title.replace(/[<>:"/\\|?*\x00-\x1f]/g, ' ').replace(/\s+/g, ' ').trim().replace(/[. ]+$/, '').slice(0, 120);
  return `Meeting - ${stem || 'notes'}.docx`;
}
