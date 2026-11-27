import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import ts from 'typescript';

const frontend = fileURLToPath(new URL('../../', import.meta.url));

function* sourceFiles(directory) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) yield* sourceFiles(file);
    else if (/\.tsx?$/.test(entry.name)) yield file;
  }
}

function registeredCommands() {
  const core = readFileSync(path.join(frontend, 'src-tauri/src/lib.rs'), 'utf8');
  const handler = /tauri::generate_handler!\[([\s\S]+?)\]\)/.exec(core);
  assert.ok(handler, 'Missing application invoke handler');
  return new Set(handler[1].replace(/\/\/[^\n]*/g, '').split(',')
    .map(entry => /([a-zA-Z_]\w*)\s*$/.exec(entry)?.[1]));
}

test('literal app commands invoked by the frontend are registered in Tauri', () => {
  const registered = registeredCommands();
  const missing = [];
  for (const file of sourceFiles(path.join(frontend, 'src'))) {
    const source = readFileSync(file, 'utf8');
    if (!source.includes('@tauri-apps/api/core')) continue;
    const tree = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true);
    const aliases = new Set();
    for (const statement of tree.statements) {
      if (!ts.isImportDeclaration(statement) || statement.moduleSpecifier.text !== '@tauri-apps/api/core') continue;
      const imports = statement.importClause?.namedBindings;
      if (!imports || !ts.isNamedImports(imports)) continue;
      for (const binding of imports.elements) {
        if ((binding.propertyName ?? binding.name).text === 'invoke') aliases.add(binding.name.text);
      }
    }
    const visit = node => {
      if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && aliases.has(node.expression.text)) {
        const command = node.arguments[0];
        if (command && ts.isStringLiteralLike(command) && !command.text.startsWith('plugin:') && !registered.has(command.text)) {
          missing.push(path.relative(frontend, file) + ': ' + command.text);
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(tree);
  }
  assert.deepEqual(missing, []);
});

test('retired commands are absent and their current entry points are registered', () => {
  const registered = registeredCommands();
  const retired = [
    'api_clear_openai_auth_config', 'api_get_openai_auth_status', 'api_process_transcript',
    'api_search_library', 'api_search_transcripts', 'export_to_planner', 'get_audio_devices',
    'get_meeting_folder_path', 'get_parakeet_directml_default_unavailable',
    'get_parakeet_directml_session_disabled', 'get_parakeet_use_directml',
    'get_reviewed_meeting_actions', 'open_database_folder', 'parakeet_retry_download',
    'save_onboarding_status_cmd', 'set_meeting_series_template', 'start_recording',
    'start_recording_summary',
  ];
  assert.deepEqual(retired.filter(command => registered.has(command)), []);
  for (const command of [
    'get_parakeet_directml_status', 'get_recording_device_inventory', 'pull_ollama_model',
    'resolve_meeting_audio_file', 'start_recording_with_devices_and_meeting', 'stop_recording',
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
