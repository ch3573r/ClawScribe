import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const jsx = (type, props) => ({ type, props });
const names = value => Object.fromEntries(value.split(' ').map(name => [name, name]));
const nodes = tree => !tree || typeof tree !== 'object' ? [] : Array.isArray(tree)
  ? tree.flatMap(nodes) : [tree, ...nodes(tree.props?.children), ...nodes(tree.props?.actions)];
const content = tree => typeof tree === 'string' ? tree : Array.isArray(tree)
  ? tree.map(content).join('') : tree?.props ? content(tree.props.children) : '';
const event = (key, code, mods = {}) => ({
  key, code, preventDefault() { this.defaultPrevented = true; }, stopPropagation() {}, ...mods,
});
const savedGlobals = { startStop: 'Ctrl+Shift+F9', pauseResume: 'Ctrl+Shift+F10', toggleWindow: 'Ctrl+Shift+F11', bookmarkRecording: 'Ctrl+Shift+F8' };

function shell(t) {
  const previous = globalThis.window;
  const target = new EventTarget();
  const storage = new Map();
  let keydown;
  globalThis.window = {
    navigator: { platform: 'Win32' },
    addEventListener(name, fn) { if (name === 'keydown') keydown = fn; else target.addEventListener(name, fn); },
    removeEventListener(name, fn) { if (name === 'keydown' && fn === keydown) keydown = null; else target.removeEventListener(name, fn); },
    dispatchEvent: e => target.dispatchEvent(e),
    localStorage: {
      getItem: key => storage.get(key) ?? null,
      setItem: (key, value) => storage.set(key, value),
      removeItem: key => storage.delete(key),
    },
  };
  t.after(() => { globalThis.window = previous; });
  return e => keydown(e);
}

async function settings(press, { initialGlobals = savedGlobals, onCapture, translate = () => null } = {}) {
  const hooks = createHookHarness();
  let globals = { ...initialGlobals };
  let captureChannel = null;
  const { KeyboardShortcutsSettings } = loadTsModule('src/components/KeyboardShortcutsSettings.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@tauri-apps/api/core': { Channel: class {}, invoke: async (command, args) => {
      if (command === 'get_shortcuts') return globals;
      if (command === 'shortcut_key_characters') return args.keys.map(translate);
      if (command === 'set_shortcut_capture') {
        await onCapture?.(args.channel);
        captureChannel = args.channel;
        return 30_000;
      }
      assert.equal(command, 'set_shortcuts');
      globals = args.bindings;
      return { bindings: globals, conflicts: [] };
    } },
    '@/components/ui/button': { Button: 'button' },
    '@/components/ui/settings-layout': names('Kbd SettingsCard SettingsFooter SettingsRow SettingsStack'),
    '@/components/settings/settings-parts': { NoteRow: 'NoteRow' },
  });
  const render = () => nodes(hooks.render(KeyboardShortcutsSettings));
  render(); await flush();
  const button = label => render().find(n => n.props?.['aria-label']?.startsWith(label));
  return {
    button, render, unmount: hooks.unmount, globals: () => globals,
    async begin(label) { button(label).props.onClick(); render(); await flush(); render(); },
    capturedGlobal(action) { captureChannel?.onmessage(action); },
    isCapturing: () => Boolean(captureChannel),
    async assign(label, e) {
      const edit = button(label);
      assert.ok(edit, 'editable action: ' + label);
      edit.props.onClick(); render(); await flush(); render(); press(e); await flush(); render();
    },
  };
}

const actions = [
  ['Play or pause', ' ', 'p', 'toggle'],
  ['Back 5 seconds', 'ArrowLeft', 'j', 'seek', -5],
  ['Forward 5 seconds', 'ArrowRight', 'l', 'seek', 5],
  ['Back 15 seconds', 'ArrowLeft', 'u', 'seek', -15, { shiftKey: true }],
  ['Forward 15 seconds', 'ArrowRight', 'o', 'seek', 15, { shiftKey: true }],
  ['Slower playback', '<', 'n', 'rate', -1],
  ['Faster playback', '>', 'm', 'rate', 1],
  ['Follow playback in the transcript', 'f', 't', 'follow'],
  ['Add a bookmark at this moment', 'b', 'a', 'bookmark'],
  ['Find in transcript', 'f', 'z', 'find', undefined, { ctrlKey: true }],
];

test('player bindings survive reopening, replace all ten defaults in the listener and help, and reset independently', async t => {
  const press = shell(t);
  const ui = await settings(press);
  for (const [label, , key] of actions) {
    // A German Z key occupies the US Y position.
    await ui.assign(label, event(key, key === 'z' ? 'KeyY' : 'Key' + key.toUpperCase()));
  }
  ui.unmount();
  const reopened = await settings(press);
  for (const [label, , key] of actions) assert.ok(reopened.button(label).props['aria-label'].includes(key.toUpperCase()));
  reopened.unmount();

  function player() {
    const hooks = createHookHarness(), calls = [];
    const { usePlaybackShortcuts } = loadTsModule('src/hooks/usePlaybackShortcuts.ts', { react: hooks.react });
    const handlers = Object.fromEntries(actions.map(([, , , type]) => [type, value => calls.push([type, value.seconds ?? value.direction])]));
    const render = () => hooks.render(() => usePlaybackShortcuts(handlers));
    render();
    return { bindings: render(), calls, unmount: hooks.unmount };
  }
  const remapped = player();
  for (const [, old, key, type, value, mods] of actions) {
    const stale = event(old, undefined, mods); press(stale); assert.equal(stale.defaultPrevented, undefined);
    press(event(key, key === 'z' ? 'KeyY' : 'Key' + key.toUpperCase()));
    assert.deepEqual(remapped.calls.at(-1), [type, value]);
    for (const target of [{ tagName: 'INPUT' }, { isContentEditable: true }, { closest: () => ({}) }]) {
      const editing = event(key, undefined, { target }); press(editing); assert.equal(editing.defaultPrevented, undefined);
    }
  }
  const { PlaybackOptions } = loadTsModule('src/components/MeetingDetails/PlaybackOptions.tsx', {
    'react/jsx-runtime': { jsx, jsxs: jsx }, '@/components/ui/button': { Button: 'button' },
    '@/components/ui/popover': names('Popover PopoverTrigger PopoverContent'),
    '@/components/ui/tooltip': names('Tooltip TooltipTrigger TooltipContent'),
    '@/components/ui/settings-layout': { Kbd: 'kbd' },
  });
  const help = nodes(PlaybackOptions({ bindings: remapped.bindings })).filter(n => n.type === 'kbd').map(content);
  assert.deepEqual(help, actions.map(([, , key]) => key.toUpperCase()));
  remapped.unmount();
  const reset = await settings(press);
  reset.render().find(n => n.type === 'button' && content(n) === 'Reset player shortcuts').props.onClick();
  await flush();
  reset.unmount();
  const defaults = player();
  for (const [, old, key, type, value, mods] of actions) {
    const stale = event(key); press(stale); assert.equal(stale.defaultPrevented, undefined);
    press(event(old, undefined, mods)); assert.deepEqual(defaults.calls.at(-1), [type, value]);
  }
  defaults.unmount();
});

test('native hotkeys delivered during key capture identify their owner and release the capture guard', async t => {
  const press = shell(t);
  const ui = await settings(press);
  await ui.begin('Start or stop recording');
  assert.equal(ui.isCapturing(), true);
  ui.capturedGlobal('pauseResume');
  assert.match(content(ui.render().find(n => n.type === 'NoteRow' && n.props.tone === 'error')), /pause or resume/i);
  await flush();
  assert.equal(ui.isCapturing(), false);
  await ui.begin('Start or stop recording');
  ui.unmount();
  await flush();
  assert.equal(ui.isCapturing(), false);
});

test('rapid editor switches and unmount cannot leave an out-of-order native capture active', async t => {
  const press = shell(t);
  const gate = deferred();
  const operations = [];
  const ui = await settings(press, { onCapture: async channel => {
    operations.push(channel ? 'begin' : 'end');
    if (operations.length === 1) await gate.promise;
  } });
  await ui.begin('Start or stop recording');
  await ui.begin('Play or pause');
  ui.unmount(); await flush();
  assert.deepEqual(operations, ['begin']);
  gate.resolve(); await flush();
  assert.deepEqual(operations, ['begin', 'end', 'begin', 'end']);
  assert.equal(ui.isCapturing(), false);
});

test('global assignment uses the Windows virtual key on QWERTZ, and reset checks shifted punctuation', async t => {
  const press = shell(t);
  const ui = await settings(press, { translate: ([vk, shift]) => vk === 188 && shift ? '<' : null });
  await ui.assign('Start or stop recording', event('y', 'KeyZ', { altKey: true, keyCode: 89 }));
  assert.equal(ui.globals().startStop, 'Alt+KeyY');
  await ui.assign('Play or pause', event('y', 'KeyZ', { altKey: true, keyCode: 89 }));
  assert.match(content(ui.render().find(n => n.type === 'NoteRow' && n.props.tone === 'error')), /start or stop/i);
  await ui.assign('Slower playback', event('n', 'KeyN'));
  await ui.assign('Start or stop recording', event('<', 'Comma', { shiftKey: true, keyCode: 188 }));
  assert.equal(ui.globals().startStop, 'Shift+Comma');
  ui.render().find(n => n.type === 'button' && content(n) === 'Reset player shortcuts').props.onClick();
  await flush();
  assert.match(content(ui.render().find(n => n.type === 'NoteRow' && n.props.tone === 'error')), /start or stop/i);
  assert.match(ui.button('Slower playback').props['aria-label'], /N/);
  ui.unmount();
});

test('settings reject player, global and built-in command collisions in either direction', async t => {
  const press = shell(t);
  const ui = await settings(press);
  const error = () => content(ui.render().find(n => n.type === 'NoteRow' && n.props.tone === 'error'));
  for (const [entry, chord, owner] of [
    ['Play or pause', event('b', 'KeyB'), /bookmark/i],
    ['Play or pause', event('F9', 'F9', { ctrlKey: true, shiftKey: true }), /start or stop recording/i],
    ['Play or pause', event('k', 'KeyK', { ctrlKey: true }), /search/i],
    ['Play or pause', event('h', 'KeyH', { metaKey: true }), /replace/i],
    ['Play or pause', event('F4', 'F4', { altKey: true }), /reserved|Windows/i],
    ['Play or pause', event('S', 'KeyS', { metaKey: true, shiftKey: true }), /reserved|Windows/i],
    ['Start or stop recording', event('f', 'KeyF', { ctrlKey: true }), /find in transcript/i],
  ]) {
    await ui.assign(entry, chord); assert.match(error(), owner);
  }
  assert.match(ui.button('Play or pause').props['aria-label'], /Space/);
  await ui.assign('Play or pause', event('z', 'KeyY', { altKey: true }));
  await ui.assign('Start or stop recording', event('z', 'KeyY', { altKey: true }));
  assert.match(error(), /play or pause/i);
  assert.match(ui.button('Start or stop recording').props['aria-label'], /F9/);
  ui.unmount();
});

test('the saved key label retains characters with multi-letter uppercase spellings', async t => {
  const press = shell(t);
  const ui = await settings(press);
  await ui.assign('Play or pause', event('ß', 'Minus'));
  assert.match(ui.button('Play or pause').props['aria-label'], /ß/);
  ui.unmount();
});
