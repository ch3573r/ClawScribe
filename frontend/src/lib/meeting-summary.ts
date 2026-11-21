import type { Block, EditorSummary, MeetingSummary, Section, SummaryDataResponse } from '@/types';
import { stripSourceAnchors } from './clipboard-format';

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

export function summaryDetails(input: string | SummaryDataResponse | null | undefined) {
  let data: unknown = input;
  if (typeof input === 'string') {
    try { data = JSON.parse(input); } catch { data = null; }
  }
  const record = isRecord(data) ? data : {};
  const cache = isRecord(record.english_cache) && isRecord(record.english_cache.source) ? record.english_cache.source : {};
  const generation = isRecord(record.generation) ? record.generation : {};
  const model = generation.model ?? cache.model_name;
  const template = generation.template ?? cache.template_id;
  const structured = isRecord(record.structured_meeting_output) ? record.structured_meeting_output : {};
  const email = record.follow_up_email ?? structured.follow_up_email;
  const translation = isRecord(record.translation) ? record.translation : {};
  const reduction = isRecord(generation.reduction) ? generation.reduction : {};
  return {
    translation: typeof translation.target === 'string' && typeof translation.message === 'string'
      ? { target: translation.target, message: translation.message } : null,
    reduction: typeof reduction.source_excerpts === 'number' && typeof reduction.passes === 'number'
      ? { sourceExcerpts: reduction.source_excerpts, passes: reduction.passes, limitRetries: typeof reduction.limit_retries === 'number' ? reduction.limit_retries : 0 } : null,
    model: typeof model === 'string' ? model : '',
    template: typeof template === 'string' ? template.replace(/_/g, ' ') : '',
    email: isRecord(email) && typeof email.body_markdown === 'string' && email.body_markdown.trim()
      ? { subject: typeof email.subject === 'string' ? stripSourceAnchors(email.subject) : '', body: stripSourceAnchors(email.body_markdown) } : null,
  };
}

export function isEditorSummary(summary: MeetingSummary): summary is EditorSummary {
  return typeof summary.markdown === 'string' || Array.isArray(summary.summary_json);
}

function legacySection(key: string, value: unknown): Section | null {
  if (!isRecord(value) || !('blocks' in value)) return null;
  const blocks: Block[] = [];
  if (Array.isArray(value.blocks)) {
    value.blocks.forEach((block: unknown, index) => {
      if (!isRecord(block)) return;
      blocks.push({
        ...block,
        id: typeof block.id === 'string' && block.id ? block.id : `${key}-${index}`,
        type: typeof block.type === 'string' ? block.type : 'text',
        content: typeof block.content === 'string' ? block.content.trim() : '',
        color: 'default',
      });
    });
  }
  return { title: typeof value.title === 'string' && value.title ? value.title : key, blocks };
}

// Old databases can contain a second layer of JSON encoding, ordered section
// maps, or the MeetingNotes envelope written when a legacy summary was edited.
export function readMeetingSummary(input: string | SummaryDataResponse | null | undefined): {
  summary: MeetingSummary | null;
  meetingName?: string;
} {
  let data: SummaryDataResponse | null | undefined;
  try {
    data = typeof input === 'string' ? JSON.parse(input) : input;
  } catch {
    return { summary: null };
  }
  if (!isRecord(data)) return { summary: null };
  const meetingName = typeof data.MeetingName === 'string' ? data.MeetingName : undefined;
  const markdown = typeof data.markdown === 'string' ? data.markdown : undefined;
  if (Array.isArray(data.summary_json)) {
    return { meetingName, summary: { summary_json: data.summary_json, ...(markdown === undefined ? {} : { markdown }) } };
  }
  if (markdown) return { meetingName, summary: { markdown } };

  const entries: [string, unknown][] = isRecord(data.MeetingNotes) && Array.isArray(data.MeetingNotes.sections)
    ? data.MeetingNotes.sections.map((section: unknown, index: number) => [`section-${index + 1}`, section])
    : (Array.isArray(data._section_order) ? data._section_order : Object.keys(data))
      .filter(key => typeof key === 'string')
      .map(key => [key, data[key]]);
  const sections: [string, Section][] = [];
  for (const [key, value] of entries) {
    const section = legacySection(key, value);
    if (section) sections.push([key, section]);
  }
  return { meetingName, summary: sections.length ? Object.fromEntries(sections) : null };
}
