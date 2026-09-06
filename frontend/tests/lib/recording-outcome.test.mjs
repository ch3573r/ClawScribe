import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
const { recordingOutcome, recordingRecoveryMessage } = loadTsModule('src/lib/recording-outcome.ts');

test('completed and legacy recordings do not invent a failure', () => {
  assert.equal(recordingRecoveryMessage(recordingOutcome()), null);
});
test('failed audio and incomplete transcription remain distinct actionable states', () => {
  assert.match(recordingRecoveryMessage(recordingOutcome({audio_save_failed: true})), /Audio could not/);
  assert.match(recordingRecoveryMessage(recordingOutcome({transcription_incomplete: true})), /transcript is incomplete/);
  assert.match(recordingRecoveryMessage(recordingOutcome({audio_save_failed: true, transcription_incomplete: true})), /recovery files/);
});
