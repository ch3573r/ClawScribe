import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

test('Confluence preserves asterisks in link URLs and formats link labels', () => {
  const { markdownToConfluenceHtml } = loadTsModule('src/lib/confluenceDraft.ts');
  const html = markdownToConfluenceHtml('[**Guide**](https://example.com/a*b*c?first=1&second=2)');
  assert.ok(html.includes('<a href="https://example.com/a*b*c?first=1&amp;second=2"><strong>Guide</strong></a>'));
});

test('Confluence tables escape cells, preserve inline formatting and normalize row lengths', () => {
  const { markdownToConfluenceHtml } = loadTsModule('src/lib/confluenceDraft.ts');
  const table = '| **Name** | Note |\n| :--- | ---: |\n| Ana | <script>alert(1)</script> |\n| Ben | left \\| right | extra |\n| Sam |';
  const expected = '<table><tbody><tr><th><strong>Name</strong></th><th>Note</th></tr><tr><td>Ana</td><td>&lt;script&gt;alert(1)&lt;/script&gt;</td></tr><tr><td>Ben</td><td>left | right</td></tr><tr><td>Sam</td><td></td></tr></tbody></table>';
  assert.equal(markdownToConfluenceHtml(table), `<div data-clawscribe-confluence-draft="true">${expected}</div>`);
  assert.ok(markdownToConfluenceHtml(`Before\n${table}\n\nAfter`).includes(`<p>Before</p>\n${expected}\n<p>After</p>`));
  assert.ok(markdownToConfluenceHtml(`- Before\n${table}`).includes(`</li>\n</ul>\n${expected}`));
  assert.ok(markdownToConfluenceHtml('| not a table |').includes('<p>| not a table |</p>'));
  for (const delimiter of ['| -- |', '| --- | --- |', '| ::--- |']) {
    assert.ok(!markdownToConfluenceHtml(`| Name |\n${delimiter}`).includes('<table>'));
  }
});

test('Confluence table pipe escaping respects even and odd backslash counts', () => {
  const { markdownToConfluenceHtml } = loadTsModule('src/lib/confluenceDraft.ts');
  const html = markdownToConfluenceHtml(String.raw`| Name | Note |
| --- | --- |
| a \\| b |
| a \|`);
  assert.ok(html.includes(String.raw`<td>a \\</td><td>b</td>`));
  assert.ok(html.includes('<td>a |</td><td></td>'));
});

test('Confluence draft removes app citation targets and retains readable times', () => {
  const { buildConfluenceDraft } = loadTsModule('src/lib/confluenceDraft.ts');
  const summaryMarkdown = '- Send draft [00:12:34](#clawscribe-source-abc123)';
  const header = {
    markdown: '# Notes\n\n- Date: 2026-01-01\n- Project tags: \\*draft\\*',
    html: '<h1>Notes</h1><ul><li>Date: 2026-01-01</li><li>Project tags: *draft*</li></ul>',
  };
  const draft = buildConfluenceDraft(header, summaryMarkdown);
  assert.ok(draft.markdown.startsWith(header.markdown));
  assert.ok(draft.html.includes(header.html));
  for (const content of [draft.markdown, draft.html]) {
    assert.ok(content.includes('(00:12:34)'));
    assert.ok(!content.includes('clawscribe-source'));
  }
  assert.ok(summaryMarkdown.includes('clawscribe-source'));
});

test('Confluence rich copy preserves nested lists, quotes, code, and task status', () => {
  const { markdownToConfluenceHtml } = loadTsModule('src/lib/confluenceDraft.ts');
  const html = markdownToConfluenceHtml([
    '- [ ] Open action',
    '  1. Nested step',
    '  2. Another step',
    '- [x] Finished action',
    '',
    '> A quoted decision',
    '',
    '```html',
    '<b>**literal**</b>',
    '```',
  ].join('\n'));
  assert.match(html, /<li>☐ Open action\s*<ol>[\s\S]*<li>Another step<\/li>\s*<\/ol>\s*<\/li>/);
  assert.match(html, /<li>☑ Finished action<\/li>/);
  assert.match(html, /<blockquote>\s*<p>A quoted decision<\/p>\s*<\/blockquote>/);
  assert.ok(html.includes('<pre><code>&lt;b&gt;**literal**&lt;/b&gt;</code></pre>'));
});
