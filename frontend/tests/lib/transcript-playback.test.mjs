import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

const { indexPlaybackRanges, activePlaybackSegment } = loadTsModule('src/hooks/transcript-playback.ts');
const row = (id, timestamp, endTime) => ({ id, timestamp, endTime, text: id });

test('active segment retains chronological precedence across overlapping speakers and tied starts', () => {
  const ranges = indexPlaybackRanges([
    row('untimed', NaN, 50),
    row('first', 1, 4),
    row('contained', 1, 2),
    row('overlap', 2, 5),
    row('invalid', 6, 6),
    row('fallback', 7),
  ]);
  for (const [time, id] of [
    [undefined, null], [0, null], [1, 'first'], [3.99, 'first'], [4, 'overlap'],
    [5, null], [7, 'fallback'], [7.74, 'fallback'], [7.75, null],
  ]) {
    assert.equal(activePlaybackSegment(ranges, time), id);
  }
});

test('a late seek reads logarithmically many ranges', () => {
  const count = 10_000;
  const ranges = indexPlaybackRanges(Array.from({ length: count }, (_, n) => row(String(n), 2 * n, 2 * n + 1)));
  let reads = 0;
  const watchedRanges = new Proxy(ranges, {
    get(target, key) {
      if (/^\d+$/.test(String(key))) reads++;
      return Reflect.get(target, key);
    },
  });
  assert.equal(activePlaybackSegment(watchedRanges, 2 * (count - 1)), String(count - 1));
  assert.ok(reads <= Math.ceil(Math.log2(count)) + 1, `read ${reads} ranges`);
});
