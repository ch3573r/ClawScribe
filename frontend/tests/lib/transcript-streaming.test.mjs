import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness } from './hook-harness.mjs';

function createView() {
  const harness = createHookHarness();
  const timers = new Map();
  let nextTimer = 0;
  let useTranscriptStreaming;
  const originalSet = globalThis.setInterval;
  const originalClear = globalThis.clearInterval;
  try {
    globalThis.setInterval = callback => { const id = ++nextTimer; timers.set(id, callback); return id; };
    globalThis.clearInterval = id => timers.delete(id);
    ({ useTranscriptStreaming } = loadTsModule(fileURLToPath(new URL('../../src/hooks/useTranscriptStreaming.ts', import.meta.url)), { react: harness.react }));
  } finally {
    globalThis.setInterval = originalSet;
    globalThis.clearInterval = originalClear;
  }
  return {
    ...harness, timers,
    render: (segments, recording = true) => harness.render(() => useTranscriptStreaming(segments, recording, true)),
    tick() { for (const callback of timers.values()) callback(); },
  };
}

test('refreshing the segment array does not interrupt the display of the full utterance', () => {
  const view = createView();
  const segment = { id: 'first', text: 'Die Besprechung beginnt am Freitag.', timestamp: 0 };
  view.render([segment]);
  view.tick();
  const beforeRefresh = view.render([segment]).getDisplayText(segment);
  assert.equal(view.timers.size, 1);
  view.tick();
  assert.ok(view.render([{ ...segment }]).getDisplayText(segment).length > beforeRefresh.length);
  for (let tick = 0; tick < 60; tick++) view.tick();
  assert.equal(view.render([segment]).getDisplayText(segment), segment.text);
  assert.equal(view.timers.size, 0);
  view.unmount();
});

test('a corrected segment and a stopped recording never display an earlier partial utterance', () => {
  const view = createView();
  const segment = { id: 'first', text: 'The meeting starts on Friday.', timestamp: 0 };
  const corrected = { ...segment, text: 'The meeting starts on Monday.' };
  view.render([segment]);
  assert.equal(view.render([corrected]).getDisplayText(corrected), corrected.text);
  assert.equal(view.render([corrected], false).getDisplayText(corrected), corrected.text);
  assert.equal(view.timers.size, 0);
  view.unmount();
});
