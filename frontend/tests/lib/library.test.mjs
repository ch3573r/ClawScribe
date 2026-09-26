import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
const { parseProjectTags, matchesProjectTag, bookmarkTime, safeDocumentName } = loadTsModule(path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../src/lib/library.ts'));

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
