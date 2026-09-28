import { invoke } from '@tauri-apps/api/core';
import type { Transcript } from '@/types';
import { bookmarkTime } from '@/lib/library';

export interface MeetingExportOptions {
  content: 'summary' | 'transcript' | 'both';
  speakers: boolean;
  timestamps: boolean;
}

export function defaultExportOptions(content: MeetingExportOptions['content'] = 'summary'): MeetingExportOptions {
  return { content, speakers: true, timestamps: true };
}

export function stripSummaryTimestamps(markdown: string): string {
  return markdown.split('\n').map(line => {
    const stripped = line
      .replace(/[ \t]*\[[^\]\r\n]*\]\(#clawscribe-source-[a-f0-9]+\)/gi, '')
      .replace(/([ \t]*)\(([^()\r\n]*)\)/g, (whole, space: string, metadata: string) => {
        const parts = metadata.split(';');
        if (!parts.every(part => /^\s*[^:;]+:\s*.*$/.test(part))) return whole;
        const remaining = parts.filter(part => !/^\s*timestamp\s*:/i.test(part));
        if (remaining.length === parts.length) return whole;
        return remaining.length ? `${space}(${remaining.map(part => part.trim()).join('; ')})` : '';
      })
      .replace(/[ \t]*\[\d{2}:\d{2}(?::\d{2})?\]/g, '');
    if (stripped === line) return line;
    // Preserve Markdown indentation and line boundaries while cleaning removal gaps.
    const indent = stripped.match(/^[ \t]*/)?.[0] ?? '';
    return indent + stripped.slice(indent.length).replace(/[ \t]{2,}/g, ' ').replace(/[ \t]+([,.;:!?])/g, '$1').trimEnd();
  }).join('\n');
}

export async function readExportSummary(getMarkdown: () => Promise<string>, options: MeetingExportOptions): Promise<string> {
  const markdown = options.content === 'transcript' ? '' : await getMarkdown();
  if (options.content === 'summary' && !markdown.trim()) {
    throw new Error('Generate or save notes before exporting a summary.');
  }
  return options.timestamps ? markdown : stripSummaryTimestamps(markdown);
}

export function formatExportTranscript(transcripts: Transcript[], options: MeetingExportOptions): string {
  return transcripts.filter(segment => !segment.is_partial && segment.text.trim()).map(segment => {
    const speaker = options.speakers ? segment.speaker?.trim() : '';
    const start = segment.audio_start_time;
    const timestamp = !options.timestamps ? '' : typeof start === 'number' && Number.isFinite(start) && start >= 0
      ? bookmarkTime(start) : segment.timestamp?.trim();
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
