import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness } from './hook-harness.mjs';

const jsx = (type, props) => ({ type, props });
const flatten = node => !node || typeof node !== 'object' ? [] : Array.isArray(node)
  ? node.flatMap(flatten) : [node, ...flatten(node.props?.children)];
const text = node => typeof node === 'string' || typeof node === 'number' ? String(node)
  : Array.isArray(node) ? node.map(text).join('') : node ? text(node.props?.children) : '';

for (const restore of [false, true]) {
  test(`${restore ? 'restore' : 'backup'} displays every incomplete meeting in a persistent report`, async () => {
    const hooks = createHookHarness();
    const notifications = [];
    let refreshes = 0;
    let pickerPath = 'synthetic.zip';
    let invokeFails = false;
    let invokes = 0;
    let expectedRestore = restore;
    let report = { meetings: 3, skipped: 1, files: 2, incomplete_meetings: [
      { meeting_id: 'recovered', title: 'Recovered meeting', recovery_files_excluded: true, audio_unavailable: false, recording_folder_missing: false },
      { meeting_id: 'pending', title: 'Pending recovery', recovery_files_excluded: true, audio_unavailable: true, recording_folder_missing: false },
      { meeting_id: 'missing', title: 'Missing recording', recovery_files_excluded: false, audio_unavailable: true, recording_folder_missing: true },
    ] };
    const { LibraryBackup } = loadTsModule('src/components/LibraryBackup.tsx', {
      react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
      '@/components/ui/button': { Button: 'button' },
      '@/components/ui/dialog': Object.fromEntries(['Dialog', 'DialogContent', 'DialogHeader', 'DialogTitle', 'DialogDescription'].map(name => [name, name])),
      '@/contexts/RecordingStateContext': { useRecordingState: () => ({}) },
      '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({ refetchMeetings: async () => { refreshes++; }, refreshProjectTags: async () => { refreshes++; } }) },
      '@tauri-apps/plugin-dialog': { save: async () => pickerPath, open: async () => pickerPath },
      '@tauri-apps/api/core': { invoke: async command => {
        invokes++;
        assert.equal(command, expectedRestore ? 'restore_library' : 'backup_library');
        if (invokeFails) throw new Error('Synthetic archive failure');
        return report;
      } },
      sonner: { toast: Object.fromEntries(['success', 'warning', 'error'].map(kind => [kind, title => notifications.push({ kind, title })])) },
    });
    const render = () => hooks.render(() => LibraryBackup());
    const run = (nextRestore = restore) => {
      expectedRestore = nextRestore;
      return flatten(render()).find(node => node.type === 'button' && text(node) === (nextRestore ? 'Choose archive to restore' : 'Save backup')).props.onClick();
    };
    await run();
    assert.equal(notifications[0].kind, 'warning');
    const alert = flatten(render()).find(node => node.props?.role === 'alert');
    assert.ok(alert);
    assert.match(text(alert), /Recovered meeting: recovery files excluded/);
    assert.match(text(alert), /Pending recovery: recovery files excluded; saved audio unavailable/);
    assert.match(text(alert), /Missing recording: recording folder missing; saved audio unavailable/);
    assert.match(text(alert), /keep a separate copy/);
    assert.equal(refreshes, restore ? 2 : 0);
    // Cancelling either picker preserves the last completed report.
    pickerPath = null;
    await run(!restore);
    assert.equal(invokes, 1);
    assert.equal(text(flatten(render()).find(node => node.props?.role === 'alert')), text(alert));
    // A selected file starts a fresh operation, even when that operation fails.
    pickerPath = 'synthetic.zip'; invokeFails = true;
    await run(!restore);
    assert.equal(notifications.at(-1).kind, 'error');
    assert.equal(flatten(render()).find(node => node.props?.role === 'alert'), undefined);
    invokeFails = false;
    await run();
    assert.ok(flatten(render()).find(node => node.props?.role === 'alert'));
    // A later complete operation replaces the old warning instead of leaving it stale.
    report = { meetings: 3, files: 3, skipped: 0, incomplete_meetings: [] };
    await run();
    assert.equal(notifications.at(-1).kind, 'success');
    assert.equal(flatten(render()).find(node => node.props?.role === 'alert'), undefined);
  });
}
