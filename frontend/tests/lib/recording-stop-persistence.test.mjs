import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

test('native Stop commits its authoritative transcript before notifying the webview', () => {
  const source = readFileSync(new URL('../../src-tauri/src/audio/recording_commands.rs', import.meta.url), 'utf8');
  const stop = source.slice(source.indexOf('async fn stop_recording_session'), source.indexOf('/// Check if recording is active'));
  assert.ok(stop.indexOf('manager.library_transcripts()') < stop.indexOf('save_recording_handoff'));
  assert.ok(stop.indexOf('save_recording_handoff') < stop.indexOf('"recording-stopped"'));
  assert.ok(stop.indexOf('drop(_stop_state)') < stop.indexOf('RECORDING_JOB.lock().unwrap().take()'));
  assert.ok(stop.indexOf('RECORDING_JOB.lock().unwrap().take()') < stop.indexOf('wake_saved_recording_summary(&app, id)'));
  assert.match(stop, /"meeting_id": meeting_id/);
  assert.match(stop, /"library_save_failed": library_save_failed/);
  const ui = readFileSync(new URL('../../src/hooks/useRecordingStop.ts', import.meta.url), 'utf8');
  assert.doesNotMatch(ui, /saveMeeting|last_recording_folder_path|start_recording_summary|startAutomaticSummary/);
});
