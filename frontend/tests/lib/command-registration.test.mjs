import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import ts from 'typescript';
import { inspectInvokes, projectContracts, registeredCommands, rustCommands } from './ipc-contract.mjs';

const frontend = fileURLToPath(new URL('../../', import.meta.url));

test('frontend invokes match registered Rust command names, arguments and return shapes', () => {
  const { problems, checked, computed } = projectContracts(frontend);
  assert.deepEqual(problems, []);
  assert.ok(checked > 100, `Expected the app's invoke calls, checked ${checked}`);
  assert.ok(computed > 0, 'Expected calls through fixed command maps');
});

function fixture(source, rust) {
  const file = 'ipc-fixture.ts';
  const options = { strict: true, noLib: true };
  const host = ts.createCompilerHost(options);
  const tree = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true);
  host.getSourceFile = name => name === file ? tree : undefined;
  const program = ts.createProgram([file], options, host);
  return inspectInvokes(program, [tree], new Map(rustCommands(rust)));
}

const rustFixture = `
  #[tauri::command]
  async fn stop_recording<R: Runtime>(app: AppHandle<R>) -> Result<(), String> { Ok(()) }
  #[command]
  fn status(state: tauri::State<'_, AppState>, meeting_id: String, optional_key: Option<String>)
      -> anyhow::Result<Option<bool>> { Ok(None) }
`;

test('contract check rejects the old Stop boolean drift and stale argument names', () => {
  const { problems, checked, computed } = fixture(`
    import { invoke as invokeNative } from '@tauri-apps/api/core';
    const commands = { stop: 'stop_recording', status: 'status' } as const;
    invokeNative<boolean>(commands.stop);
    invokeNative<boolean>(commands.status, { meeting_id: 'sample' });
  `, rustFixture);
  assert.equal(checked, 2);
  assert.equal(computed, 2);
  assert.equal(problems.length, 3);
  assert.match(problems[0], /stop_recording returns .*unit.*invoked as boolean/);
  assert.match(problems[1], /status returns .*option.*invoked as boolean/);
  assert.match(problems[2], /argument keys expected \[meetingId, optionalKey\?\], got \[meeting_id\]/);
});

test('contract check resolves widened fixed-map names and validates every choice', () => {
  const { problems, checked, computed } = fixture(`
    import * as tauri from '@tauri-apps/api/core';
    const commands = { first: 'status', second: 'other_status' };
    declare const key: keyof typeof commands;
    tauri.invoke<boolean | null>(commands[key], { meetingId: 'sample' });
    tauri.invoke<void>('stop_recording');
  `, rustFixture + `
    #[command]
    fn other_status(meeting_id: String) -> Option<bool> { None }
  `);
  assert.deepEqual(problems, []);
  assert.equal(checked, 3);
  assert.equal(computed, 2);
});

test('retired commands are absent and their current entry points are registered', () => {
  const registered = registeredCommands(readFileSync(path.join(frontend, 'src-tauri/src/lib.rs'), 'utf8'));
  const retired = [
    'api_clear_openai_auth_config', 'api_get_openai_auth_status', 'api_process_transcript',
    'api_search_library', 'api_search_transcripts', 'export_to_planner', 'get_audio_devices',
    'get_meeting_folder_path', 'get_parakeet_directml_default_unavailable',
    'get_parakeet_directml_session_disabled', 'get_parakeet_use_directml',
    'get_reviewed_meeting_actions', 'parakeet_retry_download',
    'save_onboarding_status_cmd', 'set_meeting_series_template', 'start_recording',
    'start_recording_summary', 'recover_audio_from_checkpoints', 'cleanup_checkpoints',
    'release_recovered_capture', 'repair_saved_recording', 'api_save_transcript',
  ];
  assert.deepEqual(retired.filter(command => registered.has(command)), []);
  for (const command of [
    'get_parakeet_directml_status', 'get_recording_device_inventory', 'open_database_folder',
    'open_models_folder', 'open_recordings_folder', 'pull_ollama_model', 'resolve_meeting_audio_file',
    'start_recording_with_devices_and_meeting', 'stop_recording', 'recover_interrupted_recording',
  ]) {
    assert.ok(registered.has(command), `${command} must stay registered`);
  }
});

test('saved audio uses dynamic asset grants and the webview cannot connect to Ollama directly', () => {
  const { app: { security } } = JSON.parse(readFileSync(path.join(frontend, 'src-tauri/tauri.conf.json'), 'utf8'));
  assert.equal(security.assetProtocol.enable, true);
  assert.deepEqual(security.assetProtocol.scope, []);
  assert.match(security.csp['media-src'], /\bhttps?:\/\/asset\.localhost\b/);
  assert.equal(security.csp['connect-src'], "'self'");
});
