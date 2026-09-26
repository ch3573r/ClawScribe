import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

const jsx = (type, props) => ({ type, props });
const text = node => typeof node === 'string' || typeof node === 'number' ? String(node)
  : Array.isArray(node) ? node.map(text).join('') : node ? text(node.props?.children) : '';
const flatten = node => !node || typeof node !== 'object' ? [] : Array.isArray(node)
  ? node.flatMap(flatten) : [node, ...flatten(node.props?.children)];

test('bookmark changes reload once through the event and preserve the list while loading', async () => {
  const hooks = createHookHarness();
  const reads = [];
  const listeners = new Set();
  let registrations = 0;
  const { MeetingBookmarks } = loadTsModule('src/components/MeetingDetails/MeetingBookmarks.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/components/ui/button': { Button: 'button' }, sonner: { toast: { error() {} } },
    '@tauri-apps/api/core': { invoke: async command => {
      if (command === 'list_meeting_bookmarks') { const read = deferred(); reads.push(read); return read.promise; }
      for (const listener of listeners) listener();
    } },
    '@tauri-apps/api/event': { listen: async (_, callback) => {
      registrations++; listeners.add(callback); return () => listeners.delete(callback);
    } },
  });
  const render = () => hooks.render(() => MeetingBookmarks({ meetingId: 'meeting' }));
  const button = name => flatten(render()).find(node => node.type === 'button' && text(node) === name);
  render(); await flush();
  reads[0].resolve([{ id: 'mark', seconds: 10, label: 'Decision' }]); await flush();
  assert.match(text(render()), /Bookmarks \(1\)/);
  await button('Add bookmark').props.onClick(); await flush();
  assert.equal(reads.length, 2); assert.match(text(render()), /Bookmarks \(1\)/);
  assert.equal(registrations, 1);
  reads[1].resolve([{ id: 'mark', seconds: 10, label: 'Decision' }, { id: 'new', seconds: 20, label: 'Follow-up' }]); await flush();
  button('Rename').props.onClick();
  await button('Save').props.onClick(); await flush();
  assert.equal(reads.length, 3); assert.match(text(render()), /Bookmarks \(2\)/);
  reads[2].resolve([{ id: 'mark', seconds: 10, label: 'Updated' }]); await flush();
  await button('Remove').props.onClick(); await flush();
  assert.equal(reads.length, 4); assert.match(text(render()), /Bookmarks \(1\)/);
  reads[3].resolve([]); await flush();
  assert.match(text(render()), /Bookmarks \(0\)/);
  hooks.unmount(); await flush(); assert.equal(listeners.size, 0);
});

test('project tags distinguish initial loading, failure, retry, and success', async () => {
  const hooks = createHookHarness();
  const reads = [];
  const previousWindow = globalThis.window;
  let SidebarProvider;
  try {
    globalThis.window = { matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }) };
    ({ SidebarProvider } = loadTsModule('src/components/Sidebar/SidebarProvider.tsx', {
      react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
      'next/navigation': { usePathname: () => '/meetings', useRouter: () => ({}) },
      '@/lib/analytics': { trackBackendConnection() {} },
      '@/contexts/RecordingStateContext': { useRecordingState: () => ({ isRecording: false }) },
      '@tauri-apps/api/core': { invoke: async command => {
        if (command === 'list_meeting_tags') { const read = deferred(); reads.push(read); return read.promise; }
        return [];
      } },
      '@tauri-apps/api/event': { listen: async () => () => {} },
    }));
  } finally { globalThis.window = previousWindow; }
  const render = () => hooks.render(() => SidebarProvider({ children: null })).props.value;
  let state = render();
  assert.equal(state.projectTagsLoading, true); assert.equal(state.projectTagsError, null);
  render(); await flush();
  // Provider settings initialization can supersede the first read.
  for (const read of reads) read.reject(new Error('Synthetic read failure'));
  await flush(); state = render();
  assert.equal(state.projectTagsLoading, false); assert.match(state.projectTagsError, /Could not load/);
  const retry = state.refreshProjectTags(); state = render();
  assert.equal(state.projectTagsLoading, true); assert.equal(state.projectTagsError, null);
  reads.at(-1).resolve([{ meeting_id: 'meeting', tag: 'Project' }]); await retry;
  state = render();
  assert.equal(state.projectTagsLoading, false); assert.equal(state.projectTagsError, null);
  assert.equal(state.projectTags[0].tag, 'Project'); hooks.unmount();
});
