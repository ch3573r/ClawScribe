import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

const { isActiveSummary, isTerminalSummary } = loadTsModule('src/lib/summary-job-status.ts');

test('a native summary remains active until a terminal status arrives', () => {
  for (const status of ['pending', 'processing', 'summarizing', 'regenerating']) {
    assert.equal(isActiveSummary(status), true);
    assert.equal(isTerminalSummary(status), false);
  }
  for (const status of ['completed', 'failed', 'cancelled', 'skipped', 'idle']) {
    assert.equal(isActiveSummary(status), false);
    assert.equal(isTerminalSummary(status), true);
  }
});

test('a status transport error does not terminate the native job', () => {
  assert.equal(isTerminalSummary('unavailable'), false);
});

test('paused automatic notes remain attached and can be stopped', () => {
  assert.equal(isActiveSummary('deferred'), true);
  assert.equal(isTerminalSummary('deferred'), false);
});
