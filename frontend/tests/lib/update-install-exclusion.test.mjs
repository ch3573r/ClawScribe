import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

// Source-level guards for properties that need a native build to exercise: the
// install exclusion must be owned by Rust, and every durable or outbound writer
// must be admitted through the same gate the installer takes exclusively.
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
const read = file => fs.readFileSync(path.join(root, file), 'utf8');
const rust = file => read(`frontend/src-tauri/src/${file}`);

test('the install exclusion is a Rust-owned permit, never a webview resource', () => {
  const updates = rust('updates.rs');
  assert.doesNotMatch(updates, /impl tauri::Resource for UpdateInstallPermit/);
  assert.doesNotMatch(updates, /resources_table\(\)\s*\.add\(\s*permit/);
  assert.doesNotMatch(updates, /reserve_update_install|check_update_install_ready/);
  assert.match(updates, /pub async fn install_app_update\(rid: ResourceId\)/);
  assert.match(updates, /let permit = UpdateInstallPermit::acquire\(\)\?;[\s\S]*spawn_blocking\(move \|\| \{\s*let _permit = permit;/);
  const lib = rust('lib.rs');
  assert.match(lib, /updates::download_app_update/);
  assert.match(lib, /updates::install_app_update/);
  assert.doesNotMatch(lib, /reserve_update_install|check_update_install_ready/);
});

test('the page cannot reach the updater plugin and does not hold the installer', () => {
  const conf = JSON.parse(read('frontend/src-tauri/tauri.conf.json'));
  const permissions = conf.app.security.capabilities.flatMap(capability => capability.permissions ?? []);
  assert.ok(permissions.length > 0);
  assert.equal(permissions.some(permission => permission.startsWith('updater:')), false);
  const service = read('frontend/src/services/updateService.ts');
  assert.doesNotMatch(service, /\bResource\b/);
  assert.doesNotMatch(service, /candidate\.(download|install)\(/);
  assert.match(service, /invoke\('download_app_update'/);
  assert.match(service, /invoke\('install_app_update'/);
});

test('summaries, saved answers and every export write are admitted through the install gate', () => {
  assert.match(rust('summary/service.rs'), /crate::updates::admit_work\(\)\?/);
  const answers = rust('knowledge/answers.rs');
  assert.match(answers, /crate::updates::admit_work\(\)\?/);
  // A dropped lease still owes the durable cancellation write, so it keeps the guard.
  assert.match(answers, /let install = self\.install\.take\(\);[\s\S]*let _install = install;/);
  const exports = rust('exports/commands.rs');
  const lockUses = exports.match(/EXPORT_LOCK/g) ?? [];
  assert.equal(lockUses.length, 2, 'only the definition and the single guard helper take the lock');
  assert.equal((exports.match(/let _export_guard = export_guard\(\)\?;/g) ?? []).length, 7);
  assert.match(exports, /export_meeting_to_onedrive_files[\s\S]{0,300}crate::updates::admit_work\(\)\?/);
  assert.match(rust('exports/confluence.rs'), /confluence_export_page[\s\S]{0,300}crate::updates::admit_work\(\)\?/);
});

test('live context labels describe a fixed window of up to ten minutes', () => {
  for (const file of ['README.md', 'docs/architecture.md', 'docs/meeting-quality.md', 'docs/meeting-intelligence-design.md']) {
    const text = read(file);
    assert.doesNotMatch(text, /covers the (latest|last) ten minutes/i, file);
    assert.doesNotMatch(text, /Default context is the last 10 minutes/i, file);
    assert.doesNotMatch(text, /Summarize so far/, file);
  }
});
