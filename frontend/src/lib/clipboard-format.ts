/**
 * Clean clipboard content for meeting summaries and transcripts.
 *
 * Copying produces readable plain text and a small, escaped HTML version for
 * rich editors (Outlook, Teams, OneNote, Word). Neither contains Markdown
 * markup, the internal meeting id, source-link anchors or the copy time.
 * The HTML is built only from escaped text and a fixed set of tags, and links
 * are limited to http and https.
 */
import type { Transcript } from '@/types';
import { formatTranscriptTime } from '@/lib/transcriptFormatting';

export interface ClipboardContent { text: string; html: string }

export function escapeHtml(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

export function formatMeetingDate(value?: string | null): string {
  if (!value) return '';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '';
  return date.toLocaleString([], { year: 'numeric', month: 'long', day: 'numeric', hour: '2-digit', minute: '2-digit' });
}

const SOURCE_LINK = /\[([^\]\n]*)\]\(#clawscribe-source-[0-9a-f]+\)/gi;
const MARKDOWN_ESCAPES = /\\([\\`*_{}[\]()#+\-.!|>~])/g;

/** Source citations become their plain label; their internal anchor is dropped. */
export function stripSourceAnchors(markdown: string): string {
  return markdown.replace(SOURCE_LINK, (_, label: string) => label.trim() ? `(${label.trim().replace(/^\[|\]$/g, '')})` : '');
}

function splitRow(line: string): string[] {
  const cells: string[] = [];
  let cell = '';
  for (let i = 0, body = line.trim().replace(/^\|/, ''); i < body.length; i++) {
    if (body[i] === '\\' && body[i + 1] === '|') { cell += '|'; i++; }
    else if (body[i] === '|') { cells.push(cell.trim()); cell = ''; }
    else cell += body[i];
  }
  if (cell.trim() || !cells.length) cells.push(cell.trim());
  return cells;
}
const isDelimiterRow = (line: string | undefined, columns: number) =>
  line !== undefined && line.trim().startsWith('|') && splitRow(line).length === columns && splitRow(line).every(cell => /^:?-{3,}:?$/.test(cell));

/** Inline Markdown to plain text. */
function plainInline(value: string): string {
  return value
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, (_, label: string, url: string) => label === url ? url : `${label} (${url})`)
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/(\*\*|__)(.+?)\1/g, '$2')
    .replace(/(^|[^*\w])\*(?!\s)([^*]+?)\*(?!\w)/g, '$1$2')
    .replace(/~~(.+?)~~/g, '$1')
    .replace(/`([^`]+)`/g, '$1')
    .replace(MARKDOWN_ESCAPES, '$1');
}

/** Inline Markdown to escaped HTML with a fixed tag set. */
function htmlInline(value: string): string {
  value = value.replace(/\u0000/g, '');
  const links: string[] = [];
  // Links are lifted out first so their address is never rewritten by emphasis rules.
  let text = value.replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, (_, label: string, url: string) => {
      links.push(`<a href="${escapeHtml(url)}">${htmlInline(label)}</a>`);
      return `\u0000${links.length - 1}\u0000`;
    })
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1');
  text = escapeHtml(text.replace(MARKDOWN_ESCAPES, '$1'))
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/(\*\*|__)(.+?)\1/g, '<strong>$2</strong>')
    .replace(/(^|[^*\w])\*(?!\s)([^*]+?)\*(?!\w)/g, '$1<em>$2</em>')
    .replace(/~~(.+?)~~/g, '<s>$1</s>');
  return text.replace(/\u0000(\d+)\u0000/g, (_, index: string) => links[Number(index)] ?? '');
}

const BULLET = /^(\s*)[-*+]\s+(?:\[( |x|X)\]\s+)?(.*)$/;
const NUMBERED = /^(\s*)(\d+)[.)]\s+(.*)$/;

/** Summary Markdown as readable plain text. */
export function markdownToPlainText(markdown: string): string {
  const lines = stripSourceAnchors(markdown).replace(/\r\n?/g, '\n').split('\n');
  const out: string[] = [];
  for (let index = 0; index < lines.length; index++) {
    const line = lines[index].trimEnd();
    const trimmed = line.trim();
    if (!trimmed) { if (out.length && out[out.length - 1] !== '') out.push(''); continue; }
    if (/^[-*_]{3,}$/.test(trimmed)) continue;
    if (trimmed.startsWith('|')) {
      const header = splitRow(trimmed);
      if (isDelimiterRow(lines[index + 1], header.length)) {
        out.push(header.map(plainInline).join(' | '));
        index++;
        while (lines[index + 1]?.trim().startsWith('|')) out.push(splitRow(lines[++index]).map(plainInline).join(' | '));
        continue;
      }
    }
    const heading = /^#{1,6}\s+(.*?)\s*#*$/.exec(trimmed);
    if (heading) { if (out.length && out[out.length - 1] !== '') out.push(''); out.push(plainInline(heading[1])); continue; }
    const bullet = BULLET.exec(line);
    if (bullet) {
      const depth = Math.min(4, Math.floor(bullet[1].replace(/\t/g, '  ').length / 2));
      const mark = bullet[2] === undefined ? '-' : bullet[2] === ' ' ? '☐' : '☑';
      out.push(`${'  '.repeat(depth)}${mark} ${plainInline(bullet[3])}`);
      continue;
    }
    const numbered = NUMBERED.exec(line);
    if (numbered) { out.push(`${'  '.repeat(Math.min(4, Math.floor(numbered[1].length / 2)))}${numbered[2]}. ${plainInline(numbered[3])}`); continue; }
    out.push(plainInline(trimmed.replace(/^>\s?/, '')));
  }
  while (out[out.length - 1] === '') out.pop();
  return out.join('\n');
}

/** Summary Markdown as escaped HTML. */
export function markdownToSafeHtml(markdown: string): string {
  const lines = stripSourceAnchors(markdown).replace(/\r\n?/g, '\n').split('\n');
  const html: string[] = [];
  // A nested list sits inside the item it belongs to, so an item stays open until its next sibling or the end of its list.
  const open: { tag: 'ul' | 'ol'; depth: number }[] = [];
  const closeTo = (depth: number) => { while (open.length && open[open.length - 1].depth > depth) html.push(`</li></${open.pop()!.tag}>`); };
  const item = (tag: 'ul' | 'ol', depth: number, content: string) => {
    closeTo(depth);
    const top = open[open.length - 1];
    if (!top || top.depth < depth) { open.push({ tag, depth }); html.push(`<${tag}>`); }
    else if (top.tag !== tag) { html.push(`</li></${open.pop()!.tag}>`, `<${tag}>`); open.push({ tag, depth }); }
    else html.push('</li>');
    html.push(`<li>${content}`);
  };
  for (let index = 0; index < lines.length; index++) {
    const line = lines[index].trimEnd();
    const trimmed = line.trim();
    if (!trimmed) { closeTo(-1); continue; }
    if (/^[-*_]{3,}$/.test(trimmed)) { closeTo(-1); continue; }
    if (trimmed.startsWith('|')) {
      const header = splitRow(trimmed);
      if (isDelimiterRow(lines[index + 1], header.length)) {
        closeTo(-1);
        index++;
        const rows = [`<tr>${header.map(cell => `<th>${htmlInline(cell)}</th>`).join('')}</tr>`];
        while (lines[index + 1]?.trim().startsWith('|')) {
          const cells = splitRow(lines[++index]);
          rows.push(`<tr>${header.map((_, column) => `<td>${htmlInline(cells[column] ?? '')}</td>`).join('')}</tr>`);
        }
        html.push(`<table><tbody>${rows.join('')}</tbody></table>`);
        continue;
      }
    }
    const heading = /^(#{1,6})\s+(.*?)\s*#*$/.exec(trimmed);
    if (heading) { closeTo(-1); html.push(`<h${heading[1].length}>${htmlInline(heading[2])}</h${heading[1].length}>`); continue; }
    const bullet = BULLET.exec(line);
    if (bullet) {
      const mark = bullet[2] === undefined ? '' : bullet[2] === ' ' ? '☐ ' : '☑ ';
      item('ul', Math.floor(bullet[1].replace(/\t/g, '  ').length / 2), mark + htmlInline(bullet[3]));
      continue;
    }
    const numbered = NUMBERED.exec(line);
    if (numbered) { item('ol', Math.floor(numbered[1].length / 2), htmlInline(numbered[3])); continue; }
    closeTo(-1);
    html.push(`<p>${htmlInline(trimmed.replace(/^>\s?/, ''))}</p>`);
  }
  closeTo(-1);
  return html.join('\n');
}

const heading = (title: string, date: string) => ({
  text: [title, date].filter(Boolean).join('\n'),
  html: `<h1>${escapeHtml(title)}</h1>${date ? `<p>${escapeHtml(date)}</p>` : ''}`,
});

export function summaryClipboard({ title, createdAt, markdown }: { title: string; createdAt?: string | null; markdown: string }): ClipboardContent {
  const top = heading(title.trim() || 'Meeting summary', formatMeetingDate(createdAt));
  return {
    text: `${top.text}\n\n${markdownToPlainText(markdown)}`,
    html: `${top.html}\n${markdownToSafeHtml(markdown)}`,
  };
}

export function transcriptClipboard({ title, createdAt, transcripts, includeSpeaker = true }: {
  title: string; createdAt?: string | null; transcripts: readonly Transcript[]; includeSpeaker?: boolean;
}): ClipboardContent {
  const top = heading(`${title.trim() || 'Meeting'} – transcript`, formatMeetingDate(createdAt));
  const rows = transcripts.filter(row => row.text.trim()).map(row => {
    const time = formatTranscriptTime(row.audio_start_time, row.timestamp);
    const speaker = includeSpeaker ? row.speaker?.trim() : '';
    return { time, speaker: speaker ? `${speaker}: ` : '', text: row.text.trim() };
  });
  return {
    text: `${top.text}\n\n${rows.map(row => `${row.time} ${row.speaker}${row.text}`).join('\n')}`,
    html: `${top.html}\n${rows.map(row => `<p><strong>${escapeHtml(row.time)}</strong> ${row.speaker ? `<em>${escapeHtml(row.speaker.slice(0, -2))}</em>: ` : ''}${escapeHtml(row.text)}</p>`).join('\n')}`,
  };
}

/** Writes plain text and HTML together; plain text only where rich copy is unavailable. */
export async function writeClipboard(content: ClipboardContent): Promise<'rich' | 'text'> {
  const host = globalThis as { ClipboardItem?: new (items: Record<string, Blob>) => ClipboardItem; navigator: Navigator; Blob: typeof Blob };
  const ClipboardItemConstructor = host.ClipboardItem;
  const clipboard = host.navigator.clipboard;
  if (clipboard?.write && ClipboardItemConstructor) {
    try {
      await clipboard.write([new ClipboardItemConstructor({
        'text/plain': new host.Blob([content.text], { type: 'text/plain' }),
        'text/html': new host.Blob([content.html], { type: 'text/html' }),
      })]);
      return 'rich';
    } catch { /* fall through to plain text */ }
  }
  await clipboard.writeText(content.text);
  return 'text';
}
