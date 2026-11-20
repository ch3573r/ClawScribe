import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const recording = {
  recording_id: 'meeting-1799fc19-f74b-45da-8a87-dc48704444be',
  folder_name: 'Synthetic workshop',
  first_note: 'Review the plan',
  note_count: 2,
  include_in_summary: true,
};

function findNode(node, match) {
  if (Array.isArray(node)) return node.map(child => findNode(child, match)).find(Boolean);
  if (!node || typeof node !== 'object') return undefined;
  return match(node) ? node : findNode(node.props?.children, match);
}

function setup(fail = false) {
  const hooks = createHookHarness();
  const calls = [];
  let entries = [recording];
  const { PendingRecordingNotes } = loadTsModule('src/components/PendingRecordingNotes.tsx', {
    react: hooks.react,
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push([command, args]);
      if (command === 'list_pending_recording_notes') return { entries, has_more: false };
      if (command === 'delete_pending_recording_notes') {
        if (fail) throw 'This recording has already been saved.';
        entries = [];
      }
    } },
    '@/components/ui/button': { Button: 'button' },
    '@/components/ui/dialog': Object.fromEntries(
      ['Dialog', 'DialogContent', 'DialogDescription', 'DialogFooter', 'DialogHeader', 'DialogTitle', 'DialogTrigger'].map(name => [name, name]),
    ),
  });
  const find = predicate => findNode(hooks.render(() => PendingRecordingNotes({ disabled: false })), predicate);
  const button = name => find(node => node.type === 'button' && node.props.children === name);
  const open = async () => {
    find(node => node.type === 'Dialog').props.onOpenChange(true);
    find(node => node.type === 'Dialog');
    await flush();
  };
  return { hooks, calls, open, find, button };
}

test('pending notes require selection and confirmation, and cancel leaves them available to recover', async () => {
  const ui = setup();
  try {
    await ui.open();
    assert.ok(ui.find(node => node.props?.children === 'Review the plan'));
    ui.button('Review removal').props.onClick();
    ui.button('Cancel').props.onClick();
    assert.ok(ui.button('Review removal'));
    assert.deepEqual(ui.calls.map(([command]) => command), ['list_pending_recording_notes']);
  } finally {
    ui.hooks.unmount();
  }
});

test('deleting pending notes sends only the reviewed identity and refreshes on success', async () => {
  const ui = setup();
  try {
    await ui.open();
    ui.button('Review removal').props.onClick();
    await ui.button('Delete notes and preference').props.onClick();
    ui.find(node => node.type === 'Dialog');
    await flush();
    const request = ui.calls.find(([command]) => command === 'delete_pending_recording_notes');
    assert.deepEqual({ ...request[1] }, { recordingId: recording.recording_id, noteCount: 2, includeInSummary: true });
    assert.equal(ui.button('Review removal'), undefined);
    assert.ok(ui.find(node => node.props?.children === 'No pending recording notes.'));
  } finally {
    ui.hooks.unmount();
  }
});

test('a refused deletion keeps the confirmed review visible and explains the refusal', async () => {
  const ui = setup(true);
  try {
    await ui.open();
    ui.button('Review removal').props.onClick();
    await ui.button('Delete notes and preference').props.onClick();
    assert.equal(ui.find(node => node.props?.role === 'alert').props.children, 'This recording has already been saved.');
    assert.ok(ui.button('Cancel'));
  } finally {
    ui.hooks.unmount();
  }
});
