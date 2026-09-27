import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
const { parseProjectTags, matchesProjectTag, bookmarkTime, safeDocumentName, suggestTags, addTag, removeLastTag } = loadTsModule(path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../src/lib/library.ts'));

test('tag suggestions deduplicate, rank usage then alphabetically, filter and exclude selected', () => {
  const tags = [{ meeting_id: 'a', tag: 'Zulu' }, { meeting_id: 'b', tag: 'zulu' },
    { meeting_id: 'a', tag: 'Beta' }, { meeting_id: 'a', tag: 'Atlas' }];
  assert.deepEqual(Array.from(suggestTags(tags, [], '')), ['Zulu', 'Atlas', 'Beta']);
  assert.deepEqual(Array.from(suggestTags(tags, ['ZULU'], 'A')), ['Atlas', 'Beta']);
});

test('adding tags keeps existing spelling, creates new tags and deduplicates pasted lists', () => {
  const tags = [{ meeting_id: 'a', tag: 'Project Atlas' }];
  assert.deepEqual(Array.from(addTag([], ' project   atlas ', tags)), ['Project Atlas']);
  assert.deepEqual(Array.from(addTag([], 'a, b, a', tags)), ['a', 'b']);
  assert.deepEqual(Array.from(addTag(['a'], ' New   project ', tags)), ['a', 'New project']);
});

test('tag limits reject rather than truncate and count Unicode characters', () => {
  const selected = Array.from({ length: 20 }, (_, i) => `Tag ${i}`);
  assert.throws(() => addTag(selected, 'new', []), /Maximum 20 tags/);
  assert.equal(addTag(selected, 'tag 0', []).length, 20);
  assert.throws(() => addTag([], 'x'.repeat(61), []), /60 characters/);
  assert.equal(addTag([], '😀'.repeat(60), [])[0], '😀'.repeat(60));
  assert.throws(() => addTag([], `valid, ${'x'.repeat(61)}`, []), /60 characters/);
  assert.equal(selected.length, 20);
});

test('removing the last tag leaves previous chips and handles an empty list', () => {
  assert.deepEqual(Array.from(removeLastTag(['a', 'b'])), ['a']);
  assert.deepEqual(Array.from(removeLastTag([])), []);
});

test('project tags normalize whitespace and deduplicate case', () => {
  assert.deepEqual(Array.from(parseProjectTags(' Atlas, atlas,  Customer   reviews , ,')), ['atlas', 'Customer reviews']);
  assert.deepEqual(Array.from(parseProjectTags('')), []);
});
test('project filters distinguish untagged meetings and literal reserved labels', () => {
  const tags = [{ meeting_id: 'a', tag: 'Atlas' }, { meeting_id: 'b', tag: '__untagged' }];
  assert.equal(matchesProjectTag('a', 'tag:ATLAS', tags), true);
  assert.equal(matchesProjectTag('b', 'tag:Atlas', tags), false);
  assert.equal(matchesProjectTag('c', '__untagged', tags), true);
  assert.equal(matchesProjectTag('b', '__untagged', tags), false);
  assert.equal(matchesProjectTag('b', 'tag:__untagged', tags), true);
  assert.equal(matchesProjectTag('c', '', tags), true);
});
test('bookmark times retain hours and document names avoid Windows special names', () => {
  assert.equal(bookmarkTime(7323.8), '02:02:03');
  assert.equal(safeDocumentName('CON'), 'Meeting - CON.docx');
  assert.equal(safeDocumentName('Review: A/B?'), 'Meeting - Review A B.docx');
});
