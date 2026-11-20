import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

const { findRanges, findTranscriptMatches, stepMatch, splitForHighlight, normalizeFindQuery, MAX_FIND_QUERY_LENGTH } = loadTsModule('src/lib/transcript-find.ts');
const plain = value => JSON.parse(JSON.stringify(value));

test('find is literal, case-insensitive and Unicode aware', () => {
  assert.deepEqual(plain(findRanges('Über Überprüfung über', 'über')), [{ start: 0, end: 4 }, { start: 5, end: 9 }, { start: 17, end: 21 }]);
  assert.deepEqual(plain(findRanges('a.b (c) [d] a.b', 'a.b')), [{ start: 0, end: 3 }, { start: 12, end: 15 }], 'regular expression characters are plain text');
  assert.deepEqual(plain(findRanges('anything', '(')), []);
  assert.deepEqual(plain(findRanges('anything', '   ')), []);
  assert.deepEqual(plain(findRanges('Straße ist STRASSE', 'straße')), [{ start: 0, end: 6 }]);
  assert.equal(normalizeFindQuery('  two   words  '), 'two words');
  assert.equal(normalizeFindQuery('x'.repeat(MAX_FIND_QUERY_LENGTH + 50)).length, MAX_FIND_QUERY_LENGTH);
});

test('find survives characters whose lower case changes the string length', () => {
  // U+0130 lower-cases to two code units; positions must still map to the original text.
  const text = 'İstanbul İSTANBUL';
  const ranges = plain(findRanges(text, 'stanbul'));
  assert.ok(ranges.length >= 1);
  for (const { start, end } of ranges) assert.equal(text.slice(start, end).toLocaleLowerCase().includes('stanbul'), true);
});

test('matches are reported per passage in transcript order with their position', () => {
  const segments = [
    { id: 'a', text: 'Budget for the quarter' },
    { id: 'b', text: 'Nothing here' },
    { id: 'c', text: 'budget, budget and more Budget' },
  ];
  assert.deepEqual(plain(findTranscriptMatches(segments, 'BUDGET')), [{ id: 'a', index: 0, count: 1 }, { id: 'c', index: 2, count: 3 }]);
  assert.deepEqual(plain(findTranscriptMatches(segments, '')), []);
  assert.deepEqual(plain(findTranscriptMatches(segments, 'absent')), []);
});

test('stepping wraps at both ends and starts sensibly from nothing', () => {
  assert.equal(stepMatch(-1, 3, 1), 0);
  assert.equal(stepMatch(-1, 3, -1), 2);
  assert.equal(stepMatch(2, 3, 1), 0);
  assert.equal(stepMatch(0, 3, -1), 2);
  assert.equal(stepMatch(1, 3, 1), 2);
  assert.equal(stepMatch(0, 0, 1), -1);
});

test('highlight parts rebuild the original text exactly', () => {
  const text = 'Ship the Budget, then budget again';
  const parts = plain(splitForHighlight(text, 'budget'));
  assert.equal(parts.map(part => part.text).join(''), text);
  assert.deepEqual(parts.filter(part => part.match).map(part => part.text), ['Budget', 'budget']);
  assert.deepEqual(plain(splitForHighlight(text, 'zzz')), [{ text, match: false }]);
});
