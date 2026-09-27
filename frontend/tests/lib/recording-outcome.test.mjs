import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
const { recordingOutcome, recordingRecoveryMessage, recordingBlocksAutoSummary } = loadTsModule('src/lib/recording-outcome.ts');

test('completed and legacy recordings do not invent a failure', () => {
  assert.equal(recordingRecoveryMessage(recordingOutcome()), null);
  assert.equal(recordingOutcome().recovery_files_elsewhere, false);
});

test('restored recordings direct recovery to the source computer without suggesting unavailable local audio', () => {
  for (const flags of [{}, { audio_save_failed: true, transcription_incomplete: true }]) {
    const message = recordingRecoveryMessage(recordingOutcome({ ...flags, recovery_files_elsewhere: true }));
    assert.equal(message, 'Audio could not be fully saved on the computer where this meeting was recorded. Its recovery files were not included in this backup. Recover the audio on that computer, then back it up again.');
    assert.doesNotMatch(message, /Retranscribe|Keep the meeting recovery files/);
  }
});

test('capture gaps and backup file failures do not claim playable audio failed to save', () => {
  const gaps = recordingRecoveryMessage(recordingOutcome({ capture_incomplete: true }));
  assert.match(gaps, /lost during capture/);
  assert.doesNotMatch(gaps, /disk space|Audio could not|recovery files/);
  const combined = recordingRecoveryMessage(recordingOutcome({ capture_incomplete: true, recording_files_incomplete: true }));
  assert.match(combined, /lost during capture/);
  assert.match(combined, /backup files could not be saved/);
});
test('failed audio and incomplete transcription remain distinct actionable states', () => {
  assert.match(recordingRecoveryMessage(recordingOutcome({audio_save_failed: true})), /Audio could not/);
  assert.match(recordingRecoveryMessage(recordingOutcome({transcription_incomplete: true})), /transcript is incomplete/);
  assert.match(recordingRecoveryMessage(recordingOutcome({audio_save_failed: true, transcription_incomplete: true})), /recovery files/);
});

test('auto-summary blocks only failed audio or incomplete transcription', () => {
  for (const flag of ['capture_incomplete', 'recording_files_incomplete']) {
    const outcome = recordingOutcome({ [flag]: true });
    assert.equal(recordingBlocksAutoSummary(outcome), false);
    assert.ok(recordingRecoveryMessage(outcome));
  }
  for (const flag of ['audio_save_failed', 'transcription_incomplete']) {
    assert.equal(recordingBlocksAutoSummary(recordingOutcome({ [flag]: true })), true);
  }
  assert.equal(recordingBlocksAutoSummary(null), false);
});
