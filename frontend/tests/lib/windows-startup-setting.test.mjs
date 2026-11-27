import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

function setup({ platform = 'windows', isEnabled, enable, disable }) {
  const hooks = createHookHarness();
  const jsx = (type, props) => ({ type, props });
  const { WindowsStartupSettings } = loadTsModule('src/components/WindowsStartupSettings.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/hooks/usePlatform': { usePlatform: () => platform },
    './ui/switch': { Switch: 'Switch' },
    './ui/button': { Button: 'Button' },
    './ui/settings-layout': { SettingsCard: 'SettingsCard', SettingsRow: 'SettingsRow' },
    '@tauri-apps/api/core': { invoke: command => {
      assert.equal(command, 'enable_windows_autostart');
      return enable();
    } },
    '@tauri-apps/plugin-autostart': { isEnabled, disable },
  });
  return () => hooks.render(WindowsStartupSettings);
}

function find(node, predicate) {
  if (!node || typeof node !== 'object') return undefined;
  if (Array.isArray(node)) return node.map(child => find(child, predicate)).find(Boolean);
  return predicate(node) ? node : find([node.props.children, node.props.title], predicate);
}

const toggle = node => find(node, item => item.type === 'Switch').props;

test('settings search finds sign-in startup in General', () => {
  const { filterSettingsGroups } = loadTsModule('src/components/settings/settings-sections.ts', {
    'lucide-react': {}, '@/components/IntegrationIcons': {},
  });
  for (const query of ['startup', 'Windows sign-in']) {
    const values = Array.from(filterSettingsGroups(query)).flatMap(group => Array.from(group.items, item => item.value));
    assert.deepEqual(values, ['general']);
  }
});

test('sign-in startup reads Windows state and changes it only after an explicit toggle', async () => {
  const status = deferred(), saving = deferred();
  const calls = [];
  const render = setup({
    isEnabled: () => { calls.push('read'); return status.promise; },
    enable: () => { calls.push('enable'); return saving.promise; },
    disable: async () => { calls.push('disable'); },
  });
  assert.equal(toggle(render()).disabled, true);
  status.resolve(false);
  await flush();
  assert.deepEqual(calls, ['read']);
  const loaded = toggle(render());
  assert.equal(loaded.checked, false);
  assert.equal(loaded.disabled, false);
  const update = loaded.onCheckedChange(true);
  loaded.onCheckedChange(true);
  assert.deepEqual(calls, ['read', 'enable']);
  assert.equal(toggle(render()).disabled, true);
  saving.resolve();
  await update;
  assert.equal(toggle(render()).checked, true);
  await toggle(render()).onCheckedChange(false);
  assert.equal(toggle(render()).checked, false);
  assert.deepEqual(calls, ['read', 'enable', 'disable']);
});

test('sign-in startup stays enabled if Windows rejects disabling it', async () => {
  const render = setup({
    isEnabled: async () => true,
    disable: async () => { throw new Error('Synthetic write failure'); },
  });
  render();
  await flush();
  await toggle(render()).onCheckedChange(false);
  const current = render();
  assert.equal(toggle(current).checked, true);
  assert.equal(toggle(current).disabled, false);
  assert.match(find(current, item => item.props.role === 'alert').props.children, /Could not change/);
});

test('sign-in startup is hidden and does not query autostart on other platforms', async () => {
  let reads = 0;
  const render = setup({ platform: 'linux', isEnabled: async () => { reads++; } });
  assert.equal(render(), null);
  await flush();
  assert.equal(reads, 0);
});
