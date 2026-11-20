import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

// Timers the test controls. The module under test captures the functions when it loads.
function timers() {
  const pending = new Map();
  let next = 0;
  const previous = { setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout };
  globalThis.setTimeout = (callback, delay) => { pending.set(++next, { callback, delay }); return next; };
  globalThis.clearTimeout = id => pending.delete(id);
  return {
    pending,
    fireAll() { const due = [...pending.values()]; pending.clear(); due.forEach(timer => timer.callback()); },
    restore() { Object.assign(globalThis, previous); },
  };
}

function setup(t, initial = '') {
  const clock = timers();
  t.after(() => clock.restore());
  const hooks = createHookHarness();
  const { useDraftCommit } = loadTsModule('src/hooks/useDraftCommit.ts', { react: { ...hooks.react, useCallback: hooks.react.useCallback } });
  const committed = [];
  let value = initial;
  const render = () => hooks.render(() => useDraftCommit(value, text => committed.push(text)));
  return {
    clock, committed, hooks, render,
    setValue: next => { value = next; },
  };
}

test('typing a font name changes nothing until typing pauses, then commits once', t => {
  const field = setup(t);
  let view = field.render();
  for (const text of ['C', 'Ca', 'Cal', 'Cali']) {
    view.setDraft(text);
    view = field.render();
  }
  assert.deepEqual(field.committed, [], 'no partial name reaches the settings');
  assert.equal(view.draft, 'Cali');
  assert.equal(view.pending, true);
  assert.equal(field.clock.pending.size, 1, 'one pause timer, restarted by each keystroke');
  assert.equal([...field.clock.pending.values()][0].delay, 500);
  field.clock.fireAll();
  assert.deepEqual(field.committed, ['Cali']);
});

test('Enter or leaving the field commits at once', t => {
  const field = setup(t);
  let view = field.render();
  view.setDraft('Calibri');
  view = field.render();
  view.commit();
  assert.deepEqual(field.committed, ['Calibri']);
});

test('a pending edit is committed when the field goes away', t => {
  const field = setup(t);
  let view = field.render();
  view.setDraft('Segoe UI');
  field.render();
  field.hooks.unmount();
  assert.deepEqual(field.committed, ['Segoe UI']);
});

test('nothing is committed when the text did not change', t => {
  const field = setup(t, 'Calibri');
  const view = field.render();
  view.commit();
  field.hooks.unmount();
  assert.deepEqual(field.committed, []);
  assert.equal(field.clock.pending.size, 0);
});

test('a value changed elsewhere replaces the draft', t => {
  const field = setup(t, 'Calibri');
  field.render();
  field.setValue('Cambria');
  field.render();
  assert.equal(field.render().draft, 'Cambria');
});

// The name field must not sit inside the element with role="radiogroup".
test('the installed font field is outside the interface font radio group', t => {
  const hooks = createHookHarness();
  const previousWindow = globalThis.window;
  globalThis.window = {
    addEventListener() {}, removeEventListener() {},
    matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
  };
  t.after(() => { globalThis.window = previousWindow; });
  const names = list => Object.fromEntries(list.split(' ').map(name => [name, name]));
  const jsx = (type, props) => ({ type, props });
  const { ThemeSettings } = loadTsModule('src/components/ThemeSettings.tsx', {
    react: hooks.react,
    'react/jsx-runtime': { jsx, jsxs: jsx, Fragment: 'Fragment' },
    'lucide-react': names('Monitor Moon Sparkles Sun'),
    '@/components/ui/button': names('Button'),
    '@/components/ui/input': names('Input'),
    '@/components/ui/select': names('Select SelectContent SelectItem SelectTrigger SelectValue'),
    '@/components/ui/settings-layout': names('Meta SettingsCard SettingsColumns SettingsRow StatusText'),
  });
  // Show the Installed font choice as selected.
  globalThis.window.localStorage = { getItem: key => (key === 'clawscribe.font' ? 'custom' : null), setItem() {} };
  const slotsOf = props => [props?.children, props?.aside];
  const walk = (node, visit, inside = []) => {
    if (Array.isArray(node)) return node.forEach(child => walk(child, visit, inside));
    if (!node || typeof node !== 'object') return;
    visit(node, inside);
    const here = node.props?.role === 'radiogroup' ? [...inside, node.props['aria-label']] : inside;
    slotsOf(node.props).forEach(child => walk(child, visit, here));
    if (typeof node.type === 'function' && node.type.name !== 'InstalledFontField') walk(node.type(node.props), visit, here);
  };
  const tree = hooks.render(() => ThemeSettings());
  // Second render: state from the stored settings is now in place.
  const view = hooks.render(() => ThemeSettings());
  const fields = [];
  walk(view, (node, inside) => { if (typeof node.type === 'function' && node.type.name === 'InstalledFontField') fields.push(inside); });
  void tree;
  assert.equal(fields.length, 1, 'one field is shown for the Installed font choice');
  assert.ok(!fields[0].includes('Interface font'), 'not inside the radio group');
});
