import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';

function openTraySettings(window) {
  const tray = readFileSync(new URL('../../src-tauri/src/tray.rs', import.meta.url), 'utf8');
  const [, script] = tray.match(/"settings" => \{[\s\S]*?\.eval\(("(?:[^"\\]|\\.)*")\)/);
  runInNewContext(JSON.parse(script), { window });
}

test('tray Settings uses the mounted app router without reloading', () => {
  const actions = [];
  openTraySettings({
    openSettings: () => actions.push('router'),
    location: { assign: route => actions.push(route) },
  });
  assert.deepEqual(actions, ['router']);
});

test('tray Settings navigates directly when the app router is unavailable', () => {
  const routes = [];
  openTraySettings({ location: { assign: route => routes.push(route) } });
  assert.deepEqual(routes, ['/settings']);
});
