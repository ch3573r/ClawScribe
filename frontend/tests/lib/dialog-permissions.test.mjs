import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

// Read the configuration shipped to Tauri: mocked dialog APIs cannot detect
// missing IPC permissions in the packaged desktop app.
const config = JSON.parse(readFileSync(new URL('../../src-tauri/tauri.conf.json', import.meta.url), 'utf8'));
const main = config.app.security.capabilities.find(capability => capability.identifier === 'main');

test('the local main window can save Word documents and backups and open restore archives', () => {
  assert.ok(main, 'the desktop main window needs a capability');
  assert.deepEqual(main.windows, ['main']);
  assert.notEqual(main.local, false);
  assert.equal(main.remote, undefined, 'file dialogs must not be granted to remote pages');
  for (const command of ['save', 'open']) {
    assert.ok(main.permissions.includes(`dialog:allow-${command}`), `missing permission for the ${command} file picker`);
    assert.ok(!main.permissions.includes(`dialog:deny-${command}`), `the ${command} file picker must not be denied`);
  }
});
