import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

const { captureHalted, recordingErrorMessage } = loadTsModule('src/lib/recording-capture.ts');

test('a session the core stopped after a capture error is detected', () => {
  assert.equal(captureHalted({ is_recording: true, is_paused: false, is_active: false }), true);
});

test('a normal, paused or finished session is not treated as halted', () => {
  assert.equal(captureHalted({ is_recording: true, is_paused: false, is_active: true }), false, 'recording');
  assert.equal(captureHalted({ is_recording: true, is_paused: true, is_active: false }), false, 'paused');
  assert.equal(captureHalted({ is_recording: false, is_paused: false, is_active: false }), false, 'no session');
});

test('the error message falls back to plain text when the payload is not a message', () => {
  assert.equal(recordingErrorMessage(' Audio device was disconnected '), 'Audio device was disconnected');
  assert.equal(recordingErrorMessage(null), 'Audio capture failed');
  assert.equal(recordingErrorMessage({}), 'Audio capture failed');
  assert.equal(recordingErrorMessage('  '), 'Audio capture failed');
});
