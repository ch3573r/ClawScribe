import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

const lib = loadTsModule('src/lib/shortcuts.ts');
const key = (code, mods = {}) => ({ code, ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, ...mods });

test('a key with a modifier becomes an accelerator; lone modifiers and bare keys do not', () => {
  assert.deepEqual({ ...lib.captureAccelerator(key('F9', { ctrlKey: true, shiftKey: true })) }, { kind: 'accelerator', accelerator: 'Ctrl+Shift+F9' });
  assert.deepEqual({ ...lib.captureAccelerator(key('KeyR', { metaKey: true })) }, { kind: 'accelerator', accelerator: 'Super+KeyR' });
  assert.equal(lib.captureAccelerator(key('ShiftLeft', { shiftKey: true })).kind, 'ignore');
  assert.equal(lib.captureAccelerator(key('F9')).kind, 'needs-modifier');
});

test('shortcuts are shown as separate keycaps', () => {
  assert.deepEqual([...lib.keycaps('Ctrl+Shift+F9')], ['Ctrl', 'Shift', 'F9']);
  assert.deepEqual([...lib.keycaps('Super+Alt+KeyR')], ['Win', 'Alt', 'R']);
  assert.deepEqual([...lib.keycaps('Ctrl+Digit1')], ['Ctrl', '1']);
});

test('duplicates and defaults are detected across all three actions', () => {
  const bindings = { ...lib.DEFAULT_BINDINGS };
  assert.equal(lib.duplicateAction(bindings, 'startStop', 'Ctrl+Shift+F10'), 'pauseResume');
  assert.equal(lib.duplicateAction(bindings, 'startStop', 'Ctrl+Shift+F9'), null);
  assert.equal(lib.isDefaultBindings(bindings), true);
  assert.equal(lib.isDefaultBindings({ ...bindings, toggleWindow: 'Ctrl+Alt+KeyH' }), false);
});
