import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

// Read the configuration shipped to Tauri: mocked dialog APIs cannot detect
// missing IPC permissions in the packaged desktop app.
const config = JSON.parse(readFileSync(new URL('../../src-tauri/tauri.conf.json', import.meta.url), 'utf8'));
const main = config.app.security.capabilities.find(capability => capability.identifier === 'main');

test('the window opens no file or folder dialog itself and has no direct store access: the core opens the dialogs', () => {
  assert.ok(main, 'the desktop main window needs a capability');
  assert.deepEqual(main.windows, ['main']);
  assert.notEqual(main.local, false);
  assert.equal(main.remote, undefined, 'no permission may be granted to remote pages');
  // A dialog the webview opens adds what the user picks to the asset scope and
  // can be shown by a compromised page; the core opens every picker instead.
  for (const command of ['save', 'open']) {
    assert.ok(!main.permissions.includes(`dialog:allow-${command}`), `the window must not open the ${command} picker itself`);
  }
  assert.ok(!main.permissions.some(permission => permission.startsWith('store:')), 'core-owned settings must not be writable from the window');
  // Plain message and confirm boxes are all the window keeps.
  assert.ok(main.permissions.includes('dialog:allow-message'));
});
