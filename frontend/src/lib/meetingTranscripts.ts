import type { PaginatedTranscriptsResponse, Transcript } from '@/types';
import type { invoke as nativeInvoke } from '@tauri-apps/api/core';

export interface TranscriptTextPatch {
  id: string;
  text: string;
  original_text?: string | null;
  word_timestamps?: Transcript['word_timestamps'] | null;
}

export async function fetchAllMeetingTranscripts(invoke: typeof nativeInvoke, meetingId: string): Promise<Transcript[]> {
  const transcripts: Transcript[] = [];
  const seen = new Set<string>();
  let offset = 0;

  while (true) {
    const page = await invoke<PaginatedTranscriptsResponse>('api_get_meeting_transcripts', {
      meetingId,
      limit: 1000,
      offset,
    });
    for (const transcript of page.transcripts) {
      if (!seen.has(transcript.id)) {
        seen.add(transcript.id);
        transcripts.push(transcript);
      }
    }
    if (!page.has_more) return transcripts;
    if (page.transcripts.length === 0) throw new Error('Transcript pagination did not advance');
    offset += page.transcripts.length;
  }
}
