import assert from 'node:assert/strict';
import path from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
const { planSummaryReplacement: plan } = loadTsModule(path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../src/lib/summary-text-replace.ts'));
const text = (value, styles = {}) => ({ type: 'text', text: value, styles });
const options = (query, replacement, matchCase = false) => ({ query, replacement, matchCase });

test('replaces across styled spans while preserving links, block IDs, and input', () => {
  const blocks = [{ id: 'stable', type: 'paragraph', content: [text('pro', { bold: true }), { type: 'link', href: '#clawscribe-source-abc', content: [text('ject')] }, text(' project')], children: [] }];
  const result = plan(blocks, options('project', '$&'));
  assert.equal(result.matches, 2);
  assert.deepEqual(JSON.parse(JSON.stringify(result.examples)), [{ before: 'project project', after: '$& $&' }]);
  assert.equal(result.blocks[0].content[0].text, '$&');
  assert.equal(result.blocks[0].content[0].styles.bold, true);
  assert.equal(result.blocks[0].content[1].href, '#clawscribe-source-abc');
  assert.equal(result.blocks[0].content[1].content[0].text, '');
  assert.equal(result.blocks[0].id, 'stable');
  assert.equal(blocks[0].content[0].text, 'pro');
});

test('searches nested blocks and both table cell formats, but never metadata', () => {
  const blocks = [{ id: 'Alice', props: { name: 'Alice' }, content: [text('Alice')], children: [{ content: [text('ALICE')] }] },
    { type: 'table', content: { type: 'tableContent', rows: [{ cells: [[text('Alice')], { type: 'tableCell', content: [text('Alice')] }] }] } }];
  const result = plan(blocks, options('Alice', 'Sam'));
  assert.equal(result.matches, 4);
  assert.equal(result.blocks[0].props.name, 'Alice');
  assert.equal(result.blocks[0].id, 'Alice');
  assert.equal(plan(blocks, options('Alice', 'Sam', true)).matches, 3);
});

test('uses literal search, supports German and deletion, and invalidates changed previews', () => {
  const blocks = [{ content: [text('Äpfel [x] ÄPFEL')] }];
  assert.equal(plan(blocks, options('äpfel', '')).examples[0].after, ' [x] ');
  assert.equal(plan(blocks, options('[x]', '$1')).examples[0].after, 'Äpfel $1 ÄPFEL');
  const first = plan(blocks, options('[x]', '$1'));
  assert.notEqual(first.token, plan([{ content: [text('changed [x]')] }], options('[x]', '$1')).token);
  assert.equal(plan(blocks, options('Äpfel', 'Äpfel', true)).matches, 0);
  assert.throws(() => plan(blocks, options('', 'anything')));
});
