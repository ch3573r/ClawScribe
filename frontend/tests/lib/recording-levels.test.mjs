import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

const noop = () => {};
const levels = loadTsModule('src/lib/recording-levels.ts', {
  '@tauri-apps/api/core': { invoke: async () => {} },
  '@tauri-apps/api/event': { listen: async () => noop },
});
const { applyLevels, applySilence, initialMeterState, resetMeter, SILENCE_AFTER_MS, MIC_NO_SIGNAL_AFTER_MS } = levels;
const plain = value => JSON.parse(JSON.stringify(value));
const loud = { rms: 0.1, peak: 0.3 };
const zero = { rms: 0, peak: 0 };

test('a reading is drawn on the settings meter scale, in decibels', () => {
  const state = applyLevels(initialMeterState, { mic: { rms: 0.1, peak: 0.5 }, system: loud }, 1000);
  assert.equal(state.mic.kind, 'level');
  assert.ok(Math.abs(state.mic.db - -20) < 0.01, 'rms 0.1 is -20 dB');
  assert.ok(Math.abs(state.mic.peakDb - -6.02) < 0.05, 'the peak is held at its own level');
  assert.equal(state.lastEventAt, 1000);
});

test('a source that is not captured is absent, not silent', () => {
  const state = applyLevels(initialMeterState, { mic: loud, system: null }, 1000);
  assert.deepEqual(plain(state.system), { kind: 'absent' });
  assert.equal(state.mic.kind, 'level');
  // Even after a long quiet time it stays "not captured".
  const later = applySilence(state, 1000 + 60_000);
  assert.deepEqual(plain(later.system), { kind: 'absent' });
});

test('a captured source that is silent is a level at the floor, not absent', () => {
  const state = applyLevels(initialMeterState, { mic: zero, system: zero }, 1000);
  assert.equal(state.mic.kind, 'level');
  assert.equal(state.mic.db, -60);
  assert.equal(state.system.kind, 'level');
});

test('before any event both sources are unknown', () => {
  assert.deepEqual(plain(initialMeterState.mic), { kind: 'unknown' });
  assert.deepEqual(plain(initialMeterState.system), { kind: 'unknown' });
  assert.equal(applySilence(initialMeterState, 99_999), initialMeterState, 'nothing to silence yet');
});

test('half a second without an event reads as silence', () => {
  const state = applyLevels(initialMeterState, { mic: loud, system: loud }, 1000);
  assert.equal(applySilence(state, 1000 + SILENCE_AFTER_MS - 1), state, 'not yet');
  const quiet = applySilence(state, 1000 + SILENCE_AFTER_MS);
  assert.equal(quiet.mic.db, -60);
  assert.equal(quiet.system.db, -60);
  assert.ok(quiet.mic.peakDb < state.mic.peakDb, 'the held peak falls');
  // Settled: a further tick changes nothing, so nothing is redrawn.
  let settled = quiet;
  for (let i = 0; i < 40; i++) settled = applySilence(settled, 1000 + 600 + i * 200);
  assert.equal(applySilence(settled, 1000 + 10_000), settled);
});

test('a new event after silence brings the level back', () => {
  const silent = applySilence(applyLevels(initialMeterState, { mic: loud, system: loud }, 1000), 2000);
  const back = applyLevels(silent, { mic: loud, system: loud }, 2100);
  assert.ok(back.mic.db > -60);
  assert.equal(back.mic.silentSince, null);
});

test('the held peak falls 3 dB per reading like the settings meter', () => {
  let state = applyLevels(initialMeterState, { mic: { rms: 0.5, peak: 1 }, system: null }, 0);
  assert.equal(state.mic.peakDb, 0);
  state = applyLevels(state, { mic: { rms: 0.01, peak: 0.01 }, system: null }, 100);
  assert.equal(state.mic.peakDb, -3);
});

test('a microphone that has made no sound for 10 seconds is flagged; a sound clears it', () => {
  let state = initialMeterState;
  let flagged = false;
  for (let now = 1000; now < 1000 + MIC_NO_SIGNAL_AFTER_MS; now += 100) {
    state = applyLevels(state, { mic: zero, system: loud }, now);
    flagged ||= state.micNoSignal;
  }
  assert.equal(flagged, false, 'under 10 s of silence');
  state = applyLevels(state, { mic: zero, system: loud }, 1000 + MIC_NO_SIGNAL_AFTER_MS);
  assert.equal(state.micNoSignal, true);
  state = applyLevels(state, { mic: loud, system: loud }, 1000 + MIC_NO_SIGNAL_AFTER_MS + 100);
  assert.equal(state.micNoSignal, false);
});

test('missing events count towards the silence, and a quiet room is not flagged on its own', () => {
  const start = applyLevels(initialMeterState, { mic: zero, system: zero }, 1000);
  assert.equal(applySilence(start, 1000 + MIC_NO_SIGNAL_AFTER_MS - 1).micNoSignal, false);
  assert.equal(applySilence(start, 1000 + MIC_NO_SIGNAL_AFTER_MS).micNoSignal, true);
  // Quiet room tone is above the floor: speech pauses never flag the microphone.
  let room = initialMeterState;
  for (let now = 0; now < 30_000; now += 100) room = applyLevels(room, { mic: { rms: 0.004, peak: 0.01 }, system: zero }, now);
  assert.equal(room.micNoSignal, false);
});

test('a microphone that is not captured is never flagged, and the system source never is', () => {
  let state = initialMeterState;
  for (let now = 0; now < 30_000; now += 100) state = applyLevels(state, { mic: null, system: zero }, now);
  assert.equal(state.micNoSignal, false);
});

test('pausing starts the readings again', () => {
  assert.equal(resetMeter(), initialMeterState);
});

test('once any sound was heard the microphone is never flagged for that recording', () => {
  let state = applyLevels(initialMeterState, { mic: loud, system: zero }, 0);
  assert.equal(state.micSeenSignal, true);
  // A headset that gates noise sends digital silence for minutes while the user listens.
  for (let now = 100; now <= 5 * 60_000; now += 100) {
    state = applyLevels(state, { mic: zero, system: loud }, now);
    assert.equal(state.micNoSignal, false, 'at ' + now);
  }
  // And with no events at all for minutes.
  assert.equal(applySilence(state, 10 * 60_000).micNoSignal, false);
});

test('a sound after a silent start ends the warning for good', () => {
  let state = applyLevels(initialMeterState, { mic: zero, system: zero }, 0);
  state = applyLevels(state, { mic: zero, system: zero }, MIC_NO_SIGNAL_AFTER_MS);
  assert.equal(state.micNoSignal, true);
  state = applyLevels(state, { mic: loud, system: zero }, MIC_NO_SIGNAL_AFTER_MS + 100);
  assert.equal(state.micNoSignal, false);
  state = applyLevels(state, { mic: zero, system: zero }, MIC_NO_SIGNAL_AFTER_MS + 200);
  assert.equal(applySilence(state, 5 * 60_000).micNoSignal, false, 'silent again, but it was heard');
});

test('resuming after a pause judges the microphone afresh', () => {
  let state = applyLevels(initialMeterState, { mic: loud, system: zero }, 0);
  state = resetMeter();
  assert.equal(state.micSeenSignal, false);
  state = applyLevels(state, { mic: zero, system: zero }, 1000);
  assert.equal(applyLevels(state, { mic: zero, system: zero }, 1000 + MIC_NO_SIGNAL_AFTER_MS).micNoSignal, true);
});
