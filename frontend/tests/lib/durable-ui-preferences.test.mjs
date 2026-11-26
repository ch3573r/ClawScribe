import assert from 'node:assert/strict';
import test from 'node:test';
import { deferred } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

function webview(t, entries) {
  const previous = globalThis.window;
  const storage = new Map(entries);
  const events = new EventTarget();
  globalThis.window = {
    localStorage: { getItem: key => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, value) },
    dispatchEvent: event => events.dispatchEvent(event),
  };
  t.after(() => { globalThis.window = previous; });
  return storage;
}

test('automatic checks read the native preference and mirror accepted edits for downgrade', async t => {
  const storage = webview(t, [['clawscribe.autoUpdateCheck', 'true']]);
  const invokes = [];
  const preferences = loadTsModule('src/lib/updatePreferences.ts', { '@tauri-apps/api/core': {
    invoke: async (command, args) => {
      invokes.push([command, args]);
      if (command === 'load_auto_update_preference') return false;
    },
  } });

  assert.equal(await preferences.loadAutoUpdateCheckEnabled(), false);
  assert.equal(invokes[0][1].legacyEnabled, true);
  await preferences.setAutoUpdateCheckEnabled(false);
  assert.equal(invokes[1][0], 'set_auto_update_preference');
  assert.equal(storage.get('clawscribe.autoUpdateCheck'), 'false');
});

test('playback shortcuts retain the old copy when native persistence fails', async t => {
  const storage = webview(t, [['clawscribe.playerShortcuts.v1', '{"toggle":"P"}']]);
  const bindings = loadTsModule('src/lib/player-bindings.ts', { '@tauri-apps/api/core': {
    invoke: async (command, args) => {
      if (command === 'load_player_shortcuts') {
        assert.equal(args.legacyBindings.toggle, 'P');
        return { toggle: 'K' };
      }
      throw new Error('Storage unavailable');
    },
  } });
  assert.equal((await bindings.refreshPlayerBindings()).toggle, 'K');
  await assert.rejects(bindings.savePlayerBindings({ ...bindings.DEFAULT_PLAYER_BINDINGS, toggle: 'J' }), /Storage unavailable/);
  assert.equal(JSON.parse(storage.get('clawscribe.playerShortcuts.v1')).toggle, 'P');
});

test('unavailable native reads fall back to legacy shortcuts and the update choice', async t => {
  webview(t, [['clawscribe.playerShortcuts.v1', '{"toggle":"P"}'], ['clawscribe.autoUpdateCheck', 'false']]);
  const mocks = { '@tauri-apps/api/core': { invoke: async () => { throw new Error('Unavailable'); } } };
  const bindings = loadTsModule('src/lib/player-bindings.ts', mocks);
  const updates = loadTsModule('src/lib/updatePreferences.ts', mocks);
  assert.equal((await bindings.refreshPlayerBindings()).toggle, 'P');
  assert.equal(await updates.loadAutoUpdateCheckEnabled(), false);
});

test('a late shortcut read cannot restore bindings older than a successful save', async t => {
  const storage = webview(t, []);
  const read = deferred();
  const bindings = loadTsModule('src/lib/player-bindings.ts', { '@tauri-apps/api/core': {
    invoke: async command => command === 'load_player_shortcuts' ? read.promise : undefined,
  } });
  const loading = bindings.refreshPlayerBindings();
  await bindings.savePlayerBindings({ ...bindings.DEFAULT_PLAYER_BINDINGS, toggle: 'J' });
  read.resolve({ toggle: 'K' });
  assert.equal((await loading).toggle, 'J');
  assert.equal(JSON.parse(storage.get('clawscribe.playerShortcuts.v1')).toggle, 'J');
});
