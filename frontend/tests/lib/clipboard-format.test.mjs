import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

const clip = loadTsModule('src/lib/clipboard-format.ts');
const { summaryClipboard, transcriptClipboard, markdownToPlainText, markdownToSafeHtml, stripSourceAnchors, escapeHtml, writeClipboard } = clip;

const MEETING_ID = 'meeting-3f2a9c1e-0000-4000-8000-123456789abc';
const summary = [
  '## Decisions',
  '',
  '- Ship the **new** importer [00:12:03](#clawscribe-source-9f3ab2c1)',
  '  - Keep the *old* path for one release',
  '- [ ] Ask finance',
  '- [x] Book the room',
  '',
  '1. First step',
  '2. Second step',
  '',
  '| Owner | Task |',
  '| --- | --- |',
  '| Ana | Review \\| merge |',
  '',
  'See [the plan](https://example.com/plan?a=1&b=2) and `code`.',
  '',
  '---',
  '',
  'Closing remarks',
].join('\n');

test('the summary copy is readable text with no Markdown markup, anchors or meeting id', () => {
  const { text } = summaryClipboard({ title: 'Roadmap review', createdAt: '2026-03-04T09:30:00Z', markdown: summary });
  assert.match(text, /^Roadmap review\n/);
  assert.match(text, /Ship the new importer \(00:12:03\)/);
  assert.match(text, /\n {2}- Keep the old path for one release/);
  assert.match(text, /☐ Ask finance/);
  assert.match(text, /☑ Book the room/);
  assert.match(text, /1\. First step/);
  assert.match(text, /Owner \| Task\nAna \| Review \| merge/);
  assert.match(text, /the plan \(https:\/\/example\.com\/plan\?a=1&b=2\) and code\./);
  for (const leftover of ['**', '##', '](', '`', 'clawscribe-source', '| ---', MEETING_ID, 'Meeting ID', 'Copied on', '---']) {
    assert.equal(text.includes(leftover), false, `plain text still contains ${leftover}`);
  }
});

test('the summary HTML uses a fixed tag set with escaped text and http(s) links only', () => {
  const { html } = summaryClipboard({ title: 'Roadmap review', createdAt: '2026-03-04T09:30:00Z', markdown: summary });
  assert.match(html, /^<h1>Roadmap review<\/h1>/);
  assert.match(html, /<h2>Decisions<\/h2>/);
  assert.match(html, /<strong>new<\/strong>/);
  // The nested list is inside its parent item, which closes after it.
  assert.match(html, /<li>Ship[^\n]*\n<ul>\n<li>Keep the <em>old<\/em> path for one release\n<\/li><\/ul>\n<\/li>\n<li>☐ Ask finance/);
  assert.match(html, /<ol>\n<li>First step\n<\/li>\n<li>Second step\n<\/li><\/ol>/);
  assert.match(html, /<table><tbody><tr><th>Owner<\/th><th>Task<\/th><\/tr><tr><td>Ana<\/td><td>Review \| merge<\/td><\/tr><\/tbody><\/table>/);
  assert.match(html, /<a href="https:\/\/example\.com\/plan\?a=1&amp;b=2">the plan<\/a>/);
  assert.match(html, /<code>code<\/code>/);
  for (const leftover of ['clawscribe-source', MEETING_ID, '**']) assert.equal(html.includes(leftover), false, leftover);
  assert.deepEqual([...new Set([...html.matchAll(/<\/?([a-z0-9]+)/g)].map(match => match[1]))].sort(),
    ['a', 'code', 'em', 'h1', 'h2', 'li', 'ol', 'p', 'strong', 'table', 'tbody', 'td', 'th', 'tr', 'ul']);
});

test('hostile content cannot add markup, attributes, scripts or non-web links', () => {
  const html = markdownToSafeHtml([
    '<script>alert(1)</script> & <img src=x onerror=alert(2)>',
    '[click](javascript:alert(3)) [data](data:text/html;base64,AAAA) [file](file:///C:/secret) [ok](https://example.com/a"onmouseover="x)',
    '![tracker](https://example.com/pixel.png)',
    '# <b onclick=x>title</b>',
    '- item with "quotes" and \'apostrophes\'',
  ].join('\n\n'));
  // Only the fixed tags may open; attribute-looking text stays inert because it is escaped text.
  const allowed = 'h[1-6]|p|ul|ol|li|a|strong|em|code|s|table|tbody|tr|th|td';
  assert.equal(new RegExp(`<(?!/?(?:${allowed})[\\s>])`, 'i').test(html), false, html);
  assert.equal(/href="(javascript|data|file):/i.test(html), false, html);
  const anchors = [...html.matchAll(/<a [^>]*>/g)].map(match => match[0]);
  assert.equal(anchors.length, 1);
  assert.match(anchors[0], /^<a href="https:\/\/example\.com\/a&quot;onmouseover=&quot;x">$/);
  assert.match(html, /&lt;script&gt;alert\(1\)&lt;\/script&gt; &amp; &lt;img/);
  assert.equal(html.includes('<img'), false);
  assert.equal(escapeHtml(`<a href="x">'&'</a>`), '&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;');
});

test('source anchors keep their visible label only', () => {
  assert.equal(stripSourceAnchors('Done [00:01:02](#clawscribe-source-abc123) and [](#clawscribe-source-ff).'), 'Done (00:01:02) and .');
  assert.equal(markdownToPlainText('Intro\n\n\n\nOutro\n'), 'Intro\n\nOutro');
});

test('code spans preserve underscores, Markdown markers, HTML markup, and link syntax', () => {
  // Emphasis markers inside code spans must be preserved as literals.
  assert.equal(markdownToPlainText('Call `__init__` and `**kwargs`'), 'Call __init__ and **kwargs');
  assert.match(markdownToSafeHtml('Use `__init__` here'), /<code>__init__<\/code>/);
  assert.match(markdownToSafeHtml('Pass `**kwargs` too'), /<code>\*\*kwargs<\/code>/);
  assert.match(markdownToSafeHtml('Also `~~struck~~` text'), /<code>~~struck~~<\/code>/);
  assert.equal(markdownToPlainText('Use `~~still~~` text'), 'Use ~~still~~ text');
  // HTML markup inside a code span must be escaped, not passed through.
  assert.match(markdownToSafeHtml('Use `<b>raw</b>` tags'), /<code>&lt;b&gt;raw&lt;\/b&gt;<\/code>/);
  // Link syntax inside a code span must remain literal, not create an <a> element.
  assert.match(markdownToSafeHtml('See `[link](https://example.com)` syntax'),
    /<code>\[link\]\(https:\/\/example\.com\)<\/code>/);
  assert.equal(markdownToSafeHtml('See `[link](https://example.com)` syntax').includes('<a'), false,
    'link syntax inside code span must not become an anchor');
  // Forged NUL placeholder bytes in input must not cause slot substitution.
  assert.doesNotMatch(markdownToSafeHtml(`x\u00000\u0000y`), /<a /);
  // Forged PUA sentinel bytes in input must likewise be stripped.
  assert.doesNotMatch(markdownToSafeHtml(`x0y`), /<code>/);
});

test('nested emphasis renders with balanced HTML tag stacks', () => {
  // Bold outer, italic inner: tags must nest without crossing.
  assert.match(markdownToSafeHtml('**outer *inner* end**'),
    /<p><strong>outer <em>inner<\/em> end<\/strong><\/p>/);
  // Italic outer, bold inner: same requirement.
  assert.match(markdownToSafeHtml('*outer **inner** end*'),
    /<p><em>outer <strong>inner<\/strong> end<\/em><\/p>/);
  // Triple-asterisk collapses to balanced strong+em without crossed tags.
  const triple = markdownToSafeHtml('***bold italic*** text');
  assert.match(triple, /<strong><em>bold italic<\/em><\/strong>/);
  assert.equal(/<\/strong>[^]*<\/em>/.test(triple), false, 'no </strong> before </em>');
});

test('the transcript copy has the title, date, times and speakers but no id or Markdown', () => {
  const rows = [
    { id: 'row-1', text: 'Hello <everyone>.', audio_start_time: 5, timestamp: '', speaker: 'Me' },
    { id: 'row-2', text: '  ', audio_start_time: 9, timestamp: '' },
    { id: 'row-3', text: 'Second & last.', audio_start_time: 75, timestamp: '', speaker: 'Participants' },
  ];
  const withSpeakers = transcriptClipboard({ title: 'Roadmap review', createdAt: '2026-03-04T09:30:00Z', transcripts: rows });
  assert.match(withSpeakers.text, /^Roadmap review – transcript\n/);
  assert.match(withSpeakers.text, /\n\n\[00:05\] Me: Hello <everyone>\.\n\[01:15\] Participants: Second & last\.$/);
  assert.equal(withSpeakers.text.includes(MEETING_ID), false);
  assert.equal(withSpeakers.text.includes('#'), false);
  assert.match(withSpeakers.html, /<p><strong>\[00:05\]<\/strong> <em>Me<\/em>: Hello &lt;everyone&gt;\.<\/p>/);
  assert.match(withSpeakers.html, /Second &amp; last\./);
  assert.equal(/<everyone>/.test(withSpeakers.html), false);
  const without = transcriptClipboard({ title: '', transcripts: rows, includeSpeaker: false });
  assert.match(without.text, /^Meeting – transcript\n\n\[00:05\] Hello/);
  assert.equal(without.text.includes('Me:'), false);
});

test('rich copy writes text and HTML together, and falls back to plain text', async () => {
  const content = { text: 'Plain', html: '<p>Rich</p>' };
  const written = [];
  class Item { constructor(parts) { this.parts = parts; } }
  const saved = { ClipboardItem: globalThis.ClipboardItem, navigator: Object.getOwnPropertyDescriptor(globalThis, 'navigator') };
  const setNavigator = value => Object.defineProperty(globalThis, 'navigator', { value, configurable: true });
  try {
    globalThis.ClipboardItem = Item;
    setNavigator({ clipboard: { write: async items => { written.push(items[0]); }, writeText: async text => { written.push(text); } } });
    assert.equal(await writeClipboard(content), 'rich');
    assert.deepEqual(Object.keys(written[0].parts), ['text/plain', 'text/html']);
    assert.equal(await written[0].parts['text/html'].text(), '<p>Rich</p>');
    // Rich copy refused by the platform: plain text is still copied.
    setNavigator({ clipboard: { write: async () => { throw new Error('blocked'); }, writeText: async text => { written.push(text); } } });
    assert.equal(await writeClipboard(content), 'text');
    assert.equal(written.at(-1), 'Plain');
    delete globalThis.ClipboardItem;
    assert.equal(await writeClipboard(content), 'text');
  } finally {
    if (saved.ClipboardItem) globalThis.ClipboardItem = saved.ClipboardItem; else delete globalThis.ClipboardItem;
    if (saved.navigator) Object.defineProperty(globalThis, 'navigator', saved.navigator);
  }
});
