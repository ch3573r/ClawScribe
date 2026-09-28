import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

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
  const { buildConfluenceDraftMarkdown, markdownToConfluenceHtml } = loadTsModule('src/lib/confluenceDraft.ts');
  const summaryMarkdown = '- Send draft [00:12:34](#clawscribe-source-abc123)';
  const draft = buildConfluenceDraftMarkdown({ meetingId: 'meeting', meetingTitle: 'Notes', summaryMarkdown });
  assert.ok(draft.includes('(00:12:34)'));
  assert.ok(!draft.includes('clawscribe-source'));
  assert.ok(!markdownToConfluenceHtml(draft).includes('clawscribe-source'));
  assert.ok(summaryMarkdown.includes('clawscribe-source'));
});
