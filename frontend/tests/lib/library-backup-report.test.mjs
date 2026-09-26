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
    let report = { meetings: 3, skipped: 1, files: 2, incomplete_meetings: [
      { meeting_id: 'recovered', title: 'Recovered meeting', recovery_files_excluded: true, audio_unavailable: false },
      { meeting_id: 'pending', title: 'Pending recovery', recovery_files_excluded: true, audio_unavailable: true },
    ] };
    const { LibraryBackup } = loadTsModule('src/components/LibraryBackup.tsx', {
      react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
      '@/components/ui/button': { Button: 'button' },
      '@/components/ui/dialog': Object.fromEntries(['Dialog', 'DialogContent', 'DialogHeader', 'DialogTitle', 'DialogDescription'].map(name => [name, name])),
      '@/contexts/RecordingStateContext': { useRecordingState: () => ({}) },
      '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({ refetchMeetings: async () => { refreshes++; }, refreshProjectTags: async () => { refreshes++; } }) },
      '@tauri-apps/plugin-dialog': { save: async () => 'synthetic.zip', open: async () => 'synthetic.zip' },
      '@tauri-apps/api/core': { invoke: async command => { assert.equal(command, restore ? 'restore_library' : 'backup_library'); return report; } },
      sonner: { toast: Object.fromEntries(['success', 'warning', 'error'].map(kind => [kind, title => notifications.push({ kind, title })])) },
    });
    const render = () => hooks.render(() => LibraryBackup());
    const run = () => flatten(render()).find(node => node.type === 'button' && text(node) === (restore ? 'Choose archive to restore' : 'Save backup')).props.onClick();
    await run();
    assert.equal(notifications[0].kind, 'warning');
    const alert = flatten(render()).find(node => node.props?.role === 'alert');
    assert.ok(alert);
    assert.match(text(alert), /Recovered meeting: recovery files excluded/);
    assert.match(text(alert), /Pending recovery: recovery files excluded; saved audio unavailable/);
    assert.match(text(alert), /keep a separate copy/);
    assert.equal(refreshes, restore ? 2 : 0);
    // A later complete operation replaces the old warning instead of leaving it stale.
    report = { meetings: 3, files: 3, skipped: 0, incomplete_meetings: [] };
    await run();
    assert.equal(notifications.at(-1).kind, 'success');
    assert.equal(flatten(render()).find(node => node.props?.role === 'alert'), undefined);
  });
}
