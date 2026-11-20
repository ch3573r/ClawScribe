import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

test('Quit waits on native Stop with a bounded deadline', () => {
  const exit = readFileSync(new URL('../../src-tauri/src/exit_guard.rs', import.meta.url), 'utf8');
  const path = exit.slice(exit.indexOf('async fn stop_recording_then_exit'));
  assert.match(path, /bounded_stop\(\s*EXIT_FINALIZE_TIMEOUT,\s*crate::audio::recording_commands::stop_recording/);
  assert.ok(path.indexOf('app.exit(code)') > path.indexOf('.await;'));
  assert.doesNotMatch(path, /recording-stop-complete|recording_exit_save_finished/);
});
