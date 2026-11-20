import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

const stop = loadTsModule('src/lib/stop-error.ts');

test('a short message from the core is shown as it is', () => {
  const message = 'The audio device is not responding. Try again in a moment.';
  assert.equal(stop.stopFailureMessage(message), message);
  assert.equal(stop.stopFailureMessage(new Error(`  ${message}  `)), message);
});

test('anything empty, unknown or too long becomes a plain message', () => {
  for (const unusable of [undefined, null, '', '   ', {}, 5, 'x'.repeat(301)]) {
    assert.equal(stop.stopFailureMessage(unusable), stop.STOP_FAILED_FALLBACK, String(unusable).slice(0, 20));
  }
  assert.match(stop.STOP_FAILED_FALLBACK, /still running/);
});

test('"no recording in progress" is not a failure to report', () => {
  assert.equal(stop.isNothingToStop('No recording in progress'), true);
  assert.equal(stop.isNothingToStop(new Error('No recording in progress')), true);
  assert.equal(stop.isNothingToStop(NOT_RESPONDING_TEXT()), false);
});

function NOT_RESPONDING_TEXT() { return 'The audio device is not responding.'; }
