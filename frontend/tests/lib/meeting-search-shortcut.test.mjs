import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

test('the meeting search shortcut focuses Archive or the sidebar and respects the editor', t => {
  const calls = [];
  const controls = new Map();
  let keydown;
  const previousWindow = globalThis.window;
  const previousDocument = globalThis.document;
  t.after(() => { globalThis.window = previousWindow; globalThis.document = previousDocument; });
  globalThis.window = {
    addEventListener: (_, listener) => { keydown = listener; },
    requestAnimationFrame: callback => callback(),
  };
  globalThis.document = { getElementById: id => controls.get(id) ?? null };
  const { AppShortcuts } = loadTsModule('src/components/AppShortcuts.tsx', {
    react: { useEffect: effect => effect() }, 'next/navigation': { useRouter: () => ({}) },
  });
  AppShortcuts();
  const shortcut = target => keydown({ key: 'k', ctrlKey: true, target, preventDefault() {} });
  const input = (name, hidden) => ({
    focus: () => calls.push(name), select: () => calls.push('select'), closest: () => hidden,
  });
  controls.set('meeting-search', input('sidebar', false));
  shortcut({ tagName: 'DIV' });
  controls.set('meetings-search', input('archive', true));
  controls.set('meetings-archive-tab', { click: () => calls.push('show archive') });
  shortcut({ tagName: 'DIV' });
  shortcut({ isContentEditable: true });
  assert.deepEqual(calls, ['sidebar', 'select', 'show archive', 'archive', 'select']);
});
