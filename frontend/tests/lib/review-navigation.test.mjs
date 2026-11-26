import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const jsx = (type, props) => ({ type, props });
const parts = names => Object.fromEntries(names.split(' ').map(name => [name, name]));
const nodes = tree => Array.isArray(tree) ? tree.flatMap(nodes)
  : tree && typeof tree === 'object' ? [tree, ...nodes(tree.props?.children)] : [];

test('Settings can find storage and About by topic and shares the same valid deep links', () => {
  const { filterSettingsGroups, SETTINGS_ITEMS } = loadTsModule('src/components/settings/settings-sections.ts', {
    'lucide-react': {}, '@/components/IntegrationIcons': {},
  });
  const { SETTINGS_TABS } = loadTsModule('src/components/settings/settings-tabs.ts');
  const ids = groups => Array.from(groups).flatMap(group => Array.from(group.items, item => item.value));
  assert.deepEqual(ids(filterSettingsGroups('backup storage')), ['general']);
  assert.deepEqual(ids(filterSettingsGroups('Licensing')), ['about']);
  assert.deepEqual(ids(filterSettingsGroups('connections')), ['ms365', 'calendars', 'confluence', 'webhook', 'assistant']);
  assert.deepEqual(ids(filterSettingsGroups('unknown section')), []);
  assert.deepEqual(Array.from(SETTINGS_ITEMS, item => item.value), Array.from(SETTINGS_TABS));
});

test('cancelling search discards a running response and only starts the newest queued query', async () => {
  const { createSearchLane } = loadTsModule('src/lib/search-lane.ts');
  const schedule = createSearchLane();
  const work = deferred();
  const called = [];
  const first = new AbortController();
  const second = new AbortController();
  const third = new AbortController();
  const a = schedule(first.signal, () => { called.push('first'); return work.promise; });
  first.abort();
  const b = schedule(second.signal, async () => { called.push('second'); return 'second'; });
  second.abort();
  const c = schedule(third.signal, async () => { called.push('third'); return 'third'; });
  assert.deepEqual(called, ['first']);
  assert.equal(await b, null);
  work.resolve('stale');
  assert.equal(await a, null);
  assert.equal(await c, 'third');
  assert.deepEqual(called, ['first', 'third']);
});

test('palette recording uses the shared eligibility and toggle, and Escape has no pending command', t => {
  const previousWindow = globalThis.window;
  const previousDocument = globalThis.document;
  const previousHTMLElement = globalThis.HTMLElement;
  t.after(() => Object.assign(globalThis, { window: previousWindow, document: previousDocument, HTMLElement: previousHTMLElement }));
  const listeners = new Map();
  const calls = [];
  globalThis.window = {
    addEventListener: (event, callback) => listeners.set(event, callback),
    removeEventListener: event => listeners.delete(event),
  };
  globalThis.HTMLElement = class { focus() { calls.push('focus'); } };
  globalThis.document = { activeElement: new globalThis.HTMLElement() };
  const hooks = createHookHarness();
  const sidebar = { meetings: [], handleRecordingToggle: () => calls.push('toggle'), recordingActionReason: 'Audio devices unavailable' };
  const recording = { isRecording: false };
  const { CommandPalette } = loadTsModule('src/components/CommandPalette.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx }, 'lucide-react': {},
    'next/navigation': { usePathname: () => '/', useRouter: () => ({}) },
    '@/components/ui/command': parts('Command CommandGroup CommandInput CommandItem CommandList'),
    '@/components/ui/dialog': parts('Dialog DialogContent DialogDescription DialogTitle'),
    '@/components/settings/settings-sections': { SETTINGS_ITEMS: [] },
    '@/hooks/useDebouncedDiscovery': { useDebouncedDiscovery: () => ({}) },
    '@/hooks/useKnowledgeIndexStatus': { useKnowledgeIndexStatus: () => ({ semanticReady: false }) },
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => sidebar },
    '@/contexts/RecordingStateContext': { useRecordingState: () => recording },
    '@/contexts/ImportDialogContext': { useImportDialog: () => ({}) },
  });
  const render = () => hooks.render(CommandPalette);
  const open = () => { listeners.get('shortcut:open-command-palette')(); return render(); };
  const close = tree => nodes(tree).find(node => node.type === 'DialogContent').props.onCloseAutoFocus({ preventDefault() {} });
  const recordingCommand = tree => nodes(tree).find(node => node.props?.value === 'command:recording');
  assert.equal(render().props.open, false);
  let tree = open();
  assert.equal(tree.props.open, true);
  assert.equal(recordingCommand(tree).props.disabled, true);
  tree.props.onOpenChange(false);
  close(render());
  assert.deepEqual(calls, ['focus']);
  sidebar.recordingActionReason = null;
  recording.isRecording = true;
  tree = open();
  recordingCommand(tree).props.onSelect();
  assert.equal(render().props.open, false);
  sidebar.recordingActionReason = 'Recording is stopping';
  close(render());
  assert.deepEqual(calls, ['focus', 'focus']);
  sidebar.recordingActionReason = null;
  recordingCommand(open()).props.onSelect();
  sidebar.handleRecordingToggle = () => calls.push('latest toggle');
  close(render());
  assert.deepEqual(calls, ['focus', 'focus', 'focus', 'latest toggle']);
  hooks.unmount();
});

test('Hybrid preserves literal highlights and adds only distinct transcript evidence', () => {
  const { mergeArchivePassages } = loadTsModule('src/lib/archive-hybrid-search.ts');
  const passage = (id, segments, kind = 'transcript') => ({
    meeting_id: 'meeting', text: id,
    evidence: { chunk_id: id, locator: { kind, meeting_id: 'meeting', transcript_ids: segments, start_seconds: 14 } },
  });
  const exact = { ...passage('exact', ['a']), transcript_id: 'a', highlights: [[0, 5]] };
  const results = mergeArchivePassages([exact], [
    passage('overlapping', ['a', 'b']),
    passage('semantic-only', ['c']),
    passage('document', [], 'document'),
  ]);
  assert.equal(results[0], exact);
  assert.deepEqual(Array.from(results, result => result.transcript_id), ['a', 'c']);
  assert.equal(results[1].start_seconds, 14);
});

test('bulk local export uses the core picker and stops before writing if it is cancelled', async () => {
  for (const [format, pick, write] of [
    ['markdown', 'pick_markdown_save_path', 'export_local_markdown'],
    ['word', 'pick_word_save_path', 'export_local_word'],
  ]) {
    const called = [];
    let path = null;
    const { exportArchiveMeeting } = loadTsModule('src/components/Archive/archive-bulk-export.ts', {
      '@tauri-apps/api/core': { invoke: async (command, args) => {
        called.push([command, args]);
        if (command === 'api_get_summary') return { data: { markdown: '# Saved notes' } };
        if (command === pick) return path;
      } },
    });
    const meeting = { id: 'fixture', title: 'Fixture title' };
    assert.equal(await exportArchiveMeeting(meeting, format), false);
    assert.deepEqual(called.map(([command]) => command), ['api_get_summary', pick]);
    called.length = 0;
    path = 'user-chosen-file';
    assert.equal(await exportArchiveMeeting(meeting, format), true);
    assert.deepEqual(called.map(([command]) => command), ['api_get_summary', pick, write]);
    assert.equal(called[2][1].markdown, '# Saved notes');
    assert.equal(called[2][1].includeTranscript, true);
    assert.equal(called[2][1].path, path);
  }
});

test('bulk deletion requires confirmation and defaults to keeping recording files', async () => {
  const calls = [];
  const hooks = createHookHarness();
  const { ArchiveBulkActions } = loadTsModule('src/components/Archive/ArchiveBulkActions.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/components/ui/button': parts('Button'), '@/components/ui/checkbox': parts('Checkbox'),
    '@/components/ui/dialog': parts('Dialog DialogContent DialogDescription DialogFooter DialogHeader DialogTitle'),
    '@/components/ui/input': parts('Input'),
    '@/components/ui/select': parts('Select SelectContent SelectItem SelectTrigger SelectValue'),
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({ setMeetings() {}, setCurrentMeeting() {} }) },
    '@/contexts/RecordingStateContext': { useRecordingState: () => ({}) },
    '@/services/indexedDBService': { indexedDBService: { async deleteMeeting() {} } },
    '@tauri-apps/api/core': { invoke: async (command, args) => { calls.push([command, args.meetingId, args.deleteFiles]); return {}; } },
  });
  const render = () => hooks.render(() => ArchiveBulkActions({
    action: 'delete', meetings: [{ id: 'fixture', title: 'Fixture meeting' }], onDeleted() {}, onClose() {}, onReturnFocus() {},
  }));
  const tree = render();
  assert.deepEqual(calls, []);
  assert.equal(nodes(tree).find(node => node.type === 'Checkbox').props.checked, false);
  const submit = nodes(tree).find(node => node.type === 'form').props.onSubmit;
  submit({ preventDefault() {} });
  submit({ preventDefault() {} });
  await flush();
  assert.deepEqual(calls, [['api_delete_meeting', 'fixture', false]]);
  hooks.unmount();
});
