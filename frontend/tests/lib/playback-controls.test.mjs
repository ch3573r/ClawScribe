import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness } from './hook-harness.mjs';

const controls = loadTsModule('src/lib/playback-controls.ts');
const { PLAYBACK_RATES, normalizePlaybackRate, stepPlaybackRate, formatPlaybackRate, resolvePlaybackShortcut, isTextEntry } = controls;

const element = (tagName, attributes = {}, extra = {}) => ({
  tagName, isContentEditable: false, ...extra,
  getAttribute: name => attributes[name] ?? null,
  closest: selector => extra.insideModal && selector.includes('role="dialog"') ? {} : null,
});
// The module runs in its own VM realm, so compare plain data rather than prototypes.
const plain = value => value === null ? null : JSON.parse(JSON.stringify(value));
const press = (key, target = element('BODY'), extra = {}) => plain(resolvePlaybackShortcut({ key, target, ...extra }));

test('speed steps stay on supported values, stop at the ends, and recover from bad input', () => {
  assert.deepEqual([...PLAYBACK_RATES], [0.5, 0.75, 1, 1.25, 1.5, 1.75, 2]);
  assert.equal(normalizePlaybackRate('1.25'), 1.25);
  assert.equal(normalizePlaybackRate(1.3), 1.25);
  for (const bad of [undefined, null, 'fast', NaN, Infinity, -1, 0]) assert.equal(normalizePlaybackRate(bad), 1);
  assert.equal(stepPlaybackRate(1, 1), 1.25);
  assert.equal(stepPlaybackRate(1, -1), 0.75);
  assert.equal(stepPlaybackRate(2, 1), 2);
  assert.equal(stepPlaybackRate(0.5, -1), 0.5);
  assert.equal(formatPlaybackRate(1), '1×');
  assert.equal(formatPlaybackRate(1.75), '1.75×');
});

test('playback keys map to actions on the page itself', () => {
  assert.deepEqual(press(' '), { type: 'toggle' });
  assert.deepEqual(press('ArrowLeft'), { type: 'seek', seconds: -5 });
  assert.deepEqual(press('ArrowRight'), { type: 'seek', seconds: 5 });
  assert.deepEqual(press('ArrowRight', undefined, { shiftKey: true }), { type: 'seek', seconds: 15 });
  assert.deepEqual(press('<'), { type: 'rate', direction: -1 });
  assert.deepEqual(press('>'), { type: 'rate', direction: 1 });
  assert.deepEqual(press('f'), { type: 'follow' });
  assert.deepEqual(press('B'), { type: 'bookmark' });
  assert.deepEqual(press('f', undefined, { ctrlKey: true }), { type: 'find' });
  assert.deepEqual(press('F', undefined, { metaKey: true }), { type: 'find' });
  assert.equal(press('x'), null);
});

test('the speed symbols follow the active layout without swallowing additional modifiers', () => {
  // Browser KeyboardEvent properties are prototype accessors, not own properties.
  const shiftedAngle = Object.create({ key: '<', code: 'Comma', shiftKey: true });
  assert.deepEqual(plain(resolvePlaybackShortcut(shiftedAngle)), { type: 'rate', direction: -1 });
  const ctrlAngle = Object.create({ key: '<', code: 'Comma', shiftKey: true, ctrlKey: true });
  assert.equal(resolvePlaybackShortcut(ctrlAngle), null);
  assert.deepEqual(plain(resolvePlaybackShortcut(ctrlAngle, { slower: 'Ctrl+<' })), { type: 'rate', direction: -1 });
});

test('shortcuts never steal typing, dialogs, menus or the control that owns the key', () => {
  for (const target of [
    element('INPUT'), element('TEXTAREA'), element('SELECT'),
    element('DIV', {}, { isContentEditable: true }),
    element('DIV', { role: 'textbox' }),
    element('DIV', { role: 'combobox' }),
    element('DIV', {}, { insideModal: true }),
  ]) {
    for (const key of [' ', 'ArrowLeft', 'f', 'b', '<', '>']) assert.equal(press(key, target), null, `${target.tagName} ${key}`);
    assert.equal(press('f', target, { ctrlKey: true }), null, 'find also stands down while typing');
  }
  assert.equal(isTextEntry(element('INPUT')), true);
  assert.equal(isTextEntry(element('BUTTON')), false);
  // Space activates buttons and links; arrows move sliders, tabs and menus.
  assert.equal(press(' ', element('BUTTON')), null);
  assert.equal(press(' ', element('A')), null);
  assert.equal(press(' ', element('DIV', { role: 'switch' })), null);
  assert.equal(press('ArrowLeft', element('BUTTON', { role: 'slider' })), null);
  assert.equal(press('ArrowRight', element('BUTTON', { role: 'tab' })), null);
  assert.deepEqual(press('ArrowRight', element('BUTTON')), { type: 'seek', seconds: 5 });
  // Other modifiers, key repeat, an already handled key and IME composition are ignored.
  assert.equal(press(' ', undefined, { altKey: true }), null);
  assert.equal(press('ArrowRight', undefined, { ctrlKey: true }), null);
  assert.equal(press(' ', undefined, { repeat: true }), null);
  assert.equal(press('f', undefined, { repeat: true }), null);
  assert.equal(press('ArrowRight', undefined, { defaultPrevented: true }), null);
  assert.equal(press('b', undefined, { isComposing: true }), null);
});

test('the shortcut hook listens once, calls only handlers that exist, and removes the listener', () => {
  const previousWindow = globalThis.window;
  const listeners = new Map();
  globalThis.window = { addEventListener: (name, fn) => listeners.set(name, fn), removeEventListener: name => listeners.delete(name) };
  const hooks = createHookHarness();
  try {
    const { usePlaybackShortcuts } = loadTsModule('src/hooks/usePlaybackShortcuts.ts', { react: hooks.react });
    const calls = [];
    const render = (handlers, enabled = true) => hooks.render(() => usePlaybackShortcuts(handlers, enabled));
    render({ toggle: () => calls.push('toggle'), seek: value => calls.push(value.seconds) });
    const keydown = listeners.get('keydown');
    assert.ok(keydown);
    const event = (key, extra = {}) => ({ key, target: element('BODY'), preventDefault() { this.prevented = true; }, ...extra });
    const space = event(' '); keydown(space);
    assert.deepEqual(calls, ['toggle']); assert.equal(space.prevented, true);
    keydown(event('ArrowLeft'));
    assert.deepEqual(calls, ['toggle', -5]);
    // No handler for follow: the key is left alone for the browser.
    const follow = event('f'); keydown(follow);
    assert.equal(follow.prevented, undefined);
    // A newer handler is used without re-attaching the listener.
    render({ toggle: () => calls.push('second') });
    assert.equal(listeners.get('keydown'), keydown);
    keydown(event(' '));
    assert.equal(calls.at(-1), 'second');
    const typing = event(' ', { target: element('TEXTAREA') }); keydown(typing);
    assert.equal(calls.length, 3); assert.equal(typing.prevented, undefined);
    hooks.unmount();
    assert.equal(listeners.size, 0);
  } finally { globalThis.window = previousWindow; }
});
