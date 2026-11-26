import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

test('Ctrl+K opens the command palette only outside editors and other dialogs', t => {
  const calls = [];
  let keydown, dialog = false;
  const previousWindow = globalThis.window;
  const previousDocument = globalThis.document;
  t.after(() => { globalThis.window = previousWindow; globalThis.document = previousDocument; });
  globalThis.window = {
    addEventListener: (_, listener) => { keydown = listener; },
    dispatchEvent: event => calls.push(event.type),
  };
  globalThis.document = { querySelector: () => dialog };
  const { AppShortcuts } = loadTsModule('src/components/AppShortcuts.tsx', {
    react: { useEffect: effect => effect() }, 'next/navigation': { useRouter: () => ({ push: path => calls.push(path) }) },
  });
  AppShortcuts();
  const shortcut = (target, overrides = {}) => keydown({
    key: 'k', ctrlKey: true, target, preventDefault: () => calls.push('prevented'), ...overrides,
  });
  shortcut({ tagName: 'DIV' });
  shortcut({ tagName: 'INPUT' });
  shortcut({ tagName: 'TEXTAREA' });
  shortcut({ isContentEditable: true });
  shortcut({ closest: () => ({}) });
  shortcut(null, { defaultPrevented: true });
  shortcut(null, { isComposing: true });
  shortcut(null, { repeat: true });
  dialog = true;
  shortcut(null);
  shortcut(null, { key: ',' });
  shortcut(null, { key: 'g' });
  assert.deepEqual(calls, ['prevented', 'shortcut:open-command-palette']);
  dialog = false;
  shortcut(null, { key: ',' });
  shortcut(null, { key: 'g' });
  assert.deepEqual(calls.slice(2), ['prevented', '/settings', 'prevented', 'shortcut:generate-summary']);
});
