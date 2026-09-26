import { invoke } from '@tauri-apps/api/core';
import type { Transcript } from '@/types';

export interface MeetingExportOptions {
  content: 'summary' | 'transcript' | 'both';
  speakers: boolean;
  timestamps: boolean;
}

export function defaultExportOptions(content: MeetingExportOptions['content'] = 'summary'): MeetingExportOptions {
  return { content, speakers: true, timestamps: true };
}

export async function readExportSummary(getMarkdown: () => Promise<string>, options: MeetingExportOptions): Promise<string> {
  const markdown = options.content === 'transcript' ? '' : await getMarkdown();
  if (options.content === 'summary' && !markdown.trim()) {
    throw new Error('Generate or save notes before exporting a summary.');
  }
  return markdown;
}

export function formatExportTranscript(transcripts: Transcript[], options: MeetingExportOptions): string {
  return transcripts.filter(segment => !segment.is_partial && segment.text.trim()).map(segment => {
    const speaker = options.speakers ? segment.speaker?.trim() : '';
    const timestamp = options.timestamps ? segment.timestamp?.trim() : '';
    return `${timestamp ? `[${timestamp}] ` : ''}${speaker ? `${speaker}: ` : ''}${segment.text.trim()}`;
  }).join('\n\n');
}

/** Load the full saved snapshot only when the user selects a transcript. */
export async function prepareMeetingExport(meetingId: string, getMarkdown: () => Promise<string>, options: MeetingExportOptions) {
  const summary = await readExportSummary(getMarkdown, options);
  let transcript: string | null = null;
  if (options.content !== 'summary') {
    const meeting = await invoke<{ transcripts: Transcript[] }>('api_get_meeting', { meetingId });
    transcript = formatExportTranscript(meeting.transcripts, options);
  }
  if (!summary.trim() && !transcript?.trim()) {
    throw new Error('There are no notes or transcripts to export.');
  }
  // OneNote and Confluence accept a single page body; file exports keep the
  // transcript separate so the DOCX writer treats it as plain paragraphs.
  const markdown = [summary.trim(), transcript?.trim() ? `## Transcript\n\n${transcript}` : ''].filter(Boolean).join('\n\n');
  return { summary, transcript, markdown };
}
