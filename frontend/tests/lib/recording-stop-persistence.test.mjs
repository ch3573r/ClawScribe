import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

test('native Stop commits its authoritative transcript before notifying the webview', () => {
  const source = readFileSync(new URL('../../src-tauri/src/audio/recording_commands.rs', import.meta.url), 'utf8');
  const stop = source.slice(source.indexOf('async fn stop_recording_session'), source.indexOf('/// Check if recording is active'));
  // Poison-tolerant lock; rustfmt may wrap the chain across lines.
  const jobRelease = stop.search(/RECORDING_JOB\s*\.lock\(\)\s*\.unwrap_or_else\(\|e\| e\.into_inner\(\)\)\s*\.take\(\)/);
  assert.ok(jobRelease > 0);
  assert.ok(stop.indexOf('manager.library_transcripts()') < stop.indexOf('save_recording_handoff'));
  assert.ok(stop.indexOf('save_recording_handoff') < stop.indexOf('"recording-stopped"'));
  assert.ok(stop.indexOf('"recording-stopped"') < jobRelease);
  assert.ok(jobRelease < stop.indexOf('_stop_state.close()'));
  assert.ok(stop.indexOf('_stop_state.close()') < stop.indexOf('crate::tray::start_after_save'));
  assert.ok(jobRelease < stop.indexOf('wake_saved_recording_summary(&app, id)'));
  assert.match(stop, /"meeting_id": meeting_id/);
  assert.match(stop, /"library_save_failed": library_save_failed/);
  const ui = readFileSync(new URL('../../src/hooks/useRecordingStop.ts', import.meta.url), 'utf8');
  assert.doesNotMatch(ui, /saveMeeting|last_recording_folder_path|start_recording_summary|startAutomaticSummary/);
});

test('Stop takes the recording for finalization without waiting on a driver reconnect', () => {
  const source = readFileSync(new URL('../../src-tauri/src/audio/recording_commands.rs', import.meta.url), 'utf8');
  const stop = source.slice(source.indexOf('async fn stop_recording_session'), source.indexOf('/// Check if recording is active'));
  assert.doesNotMatch(stop, /wait_for_reconnect|RECONNECT_OWNER/);
  assert.match(stop, /take_recording_for_stop\(\)/);
  assert.match(stop, /stop_streams_and_force_flush\(\)\.await/);
});

test('a drain timeout reports actual audio retention after finalization', () => {
  const source = readFileSync(new URL('../../src-tauri/src/audio/recording_commands.rs', import.meta.url), 'utf8');
  const stop = source.slice(source.indexOf('async fn stop_recording_session'), source.indexOf('/// Check if recording is active'));
  assert.ok(stop.indexOf('report.audio_path.is_some()') > stop.indexOf('.save_recording_only('));
  assert.ok(stop.indexOf('report.audio_path.is_some()') < stop.indexOf('transcription_drain_warning(remaining,'));
});

test('the main window listens for transcription warnings with a shared toast ID', () => {
  const layout = readFileSync(new URL('../../src/app/layout.tsx', import.meta.url), 'utf8');
  const effect = layout.slice(layout.indexOf('// Surface non-fatal warnings'), layout.indexOf("}>('transcription-fell-back-to-local'"));
  assert.match(effect, /listen<string>\('transcription-warning',/);
  assert.match(effect, /id: isTranscriptionWarning \? 'live-transcription-warning' : undefined/);
});

test('the desktop binary installs the panic hook before entering the app runtime', () => {
  const main = readFileSync(new URL('../../src-tauri/src/main.rs', import.meta.url), 'utf8');
  assert.match(main, /std::panic::set_hook/);
  assert.match(main, /log::error!/);
  assert.match(main, /Backtrace::force_capture/);
  assert.match(main, /fn main\(\) \{\s*install_panic_hook\(\);/);
});

test('worker dispatch accounts for a chunk only once a slot is reserved', () => {
  const worker = readFileSync(new URL('../../src-tauri/src/audio/transcription/worker.rs', import.meta.url), 'utf8');
  const dispatch = worker.slice(worker.indexOf('async fn dispatch_chunk<'), worker.indexOf('async fn receive_work<'));
  assert.ok(dispatch.includes('cancellation.cancelled()'));
  assert.ok(dispatch.indexOf('sender.reserve()') < dispatch.indexOf('chunks_queued.fetch_add('));
  assert.ok(dispatch.indexOf('chunks_queued.fetch_add(') < dispatch.indexOf('permit.send(item)'));
  assert.ok(!worker.includes('Waiting for dispatched chunks'));
});
