import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

const { startReadinessReason, recordingActionBlock } = loadTsModule('src/lib/recording-start-readiness.ts');
const ready = {
  inventoryStatus: 'ready', preferencesStatus: 'ready',
  mic: 'available', system: 'default_unavailable',
  mode: 'live', hasReadiness: true, readinessError: false, modelIssue: null,
  savingDevices: false, savingMode: false,
};

test('Home and Sidebar can start with only a default microphone, but not a missing preferred output', () => {
  assert.equal(startReadinessReason(ready), null);
  assert.equal(startReadinessReason({ ...ready, system: 'preferred_unavailable' }), 'Preferred output unavailable');
});

test('audio-only does not require a live model and unsaved preferences block Start', () => {
  const failedModel = { ...ready, hasReadiness: false, readinessError: true };
  assert.equal(startReadinessReason(failedModel), 'Could not check speech model');
  assert.equal(startReadinessReason({ ...failedModel, mode: 'audio_only' }), null);
  assert.equal(startReadinessReason({ ...ready, savingDevices: true }), 'Saving recording settings…');
});

test('the recording action stays disabled during a save, even if capture still reports active', () => {
  assert.equal(recordingActionBlock({ isRecording: true, isStopping: true }, 'Checking audio devices'), 'Saving recording…');
  assert.equal(recordingActionBlock({ isRecording: true }, 'Checking audio devices'), null);
  assert.equal(recordingActionBlock({ isStarting: true }, null), 'Starting recording…');
});
