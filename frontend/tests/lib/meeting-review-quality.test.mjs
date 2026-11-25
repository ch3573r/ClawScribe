import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

const jsx = (type, props) => ({ type, props });
const stubs = names => Object.fromEntries(names.split(' ').map(name => [name, name]));
const nodes = node => Array.isArray(node) ? node.flatMap(nodes) : node?.props ? [node, ...nodes(node.props.children)] : [];
const text = node => typeof node === 'string' || typeof node === 'number' ? String(node)
  : Array.isArray(node) ? node.map(text).join('') : node?.props ? text(node.props.children) : '';
const dialogs = stubs('Dialog DialogContent DialogHeader DialogDescription DialogTitle DialogFooter');
const dropdown = stubs('DropdownMenu DropdownMenuContent DropdownMenuItem DropdownMenuTrigger');
const button = (tree, label) => nodes(tree).find(node => node.type === 'Button' && text(node) === label);

test('bookmark times accept clock notation and seconds, and clamp to the recording', () => {
  const { parseBookmarkTime } = loadTsModule('src/lib/bookmark-time.ts');
  assert.equal(parseBookmarkTime(' 02:03 '), 123);
  assert.equal(parseBookmarkTime('61:04'), 3664);
  assert.equal(parseBookmarkTime('01:02:03'), 3723);
  assert.equal(parseBookmarkTime('12.5'), 12.5);
  assert.equal(parseBookmarkTime('01:02:03', 130.5), 130.5);
  assert.equal(parseBookmarkTime('00:00', 120), 0);
  for (const invalid of ['', '-1', '1e2', '01:60', '01:60:00', '1:2', '1:02:03:04', 'Infinity', '604801']) {
    assert.equal(parseBookmarkTime(invalid), null, invalid);
  }
});

test('a stale source takes its playback offset from the same canonical row', async () => {
  const { summarySourcePlaybackOffset } = loadTsModule('src/lib/summary-source-navigation.ts');
  const source = { transcript_id: 'row-1', transcript_index: 50, timestamp: 15, stale: false };
  const ids = [];
  const read = async index => { ids.push(index); return { id: 'row-1', audio_start_time: 25.5 }; };
  assert.equal(await summarySourcePlaybackOffset(source, read), 15);
  assert.deepEqual(ids, []);
  assert.equal(await summarySourcePlaybackOffset({ ...source, stale: true }, read), 25.5);
  assert.deepEqual(ids, [50]);
  assert.equal(await summarySourcePlaybackOffset({ ...source, stale: true }, async () => ({ id: 'other', audio_start_time: 25 })), null);
  assert.equal(await summarySourcePlaybackOffset({ ...source, stale: true }, async () => ({ id: 'row-1', audio_start_time: null })), null);
  assert.equal(await summarySourcePlaybackOffset({ ...source, stale: true }, async () => { throw new Error('unavailable'); }), null);
  assert.equal(await summarySourcePlaybackOffset({ ...source, timestamp: -1 }, read), null);
});

test('summary Find matches across formatting spans with literal, case-insensitive offsets', () => {
  const { findSummaryText } = loadTsModule('src/lib/summary-find.ts');
  const spans = ['İ ', 'Über', 'gabe [x] übergabe'];
  assert.deepEqual(JSON.parse(JSON.stringify(findSummaryText(spans, ' Übergabe '))), [
    { start: { span: 1, offset: 0 }, end: { span: 2, offset: 4 } },
    { start: { span: 2, offset: 9 }, end: { span: 2, offset: 17 } },
  ]);
  assert.deepEqual(JSON.parse(JSON.stringify(findSummaryText(spans, '[X]'))), [
    { start: { span: 2, offset: 5 }, end: { span: 2, offset: 8 } },
  ]);
  assert.equal(findSummaryText(spans, ' ').length, 0);
});

function summaryGeneration(provider, invoke, toast = { info() {}, error: message => assert.fail(message) }, onOpenModelSettings) {
  const hooks = createHookHarness();
  const { useSummaryGeneration } = loadTsModule('src/hooks/meeting-details/useSummaryGeneration.ts', {
    react: hooks.react,
    '@tauri-apps/api/core': { invoke },
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({ startSummaryPolling() {} }) },
    '@/lib/analytics': { __esModule: true, default: { async trackSummaryGenerationStarted() {} } },
    '@/lib/meetingContext': { getMeetingContext: async () => '' },
    '@/lib/meetingDrafts': { confirmSummaryRegeneration: async () => true },
    '@/lib/summary-language-preferences': { readMeetingSummaryLanguage: async () => ({ language: null }) },
    sonner: { toast },
  });
  const render = () => hooks.render(() => useSummaryGeneration({
    meeting: { id: 'test-meeting', created_at: '2026-01-01T12:00:00Z' },
    modelConfig: { provider, model: 'synthetic-model' },
    isModelConfigLoading: false, selectedTemplate: '', updateMeetingTitle() {}, setAiSummary() {}, onOpenModelSettings,
  }));
  return { render, unmount: hooks.unmount };
}

test('concurrent Ollama summary actions share one provider check and one start', async () => {
  const models = deferred();
  const commands = [];
  const { render, unmount } = summaryGeneration('ollama', async (command, args) => {
    commands.push([command, args]);
    if (command === 'get_ollama_models') return models.promise;
    if (command === 'api_generate_meeting_summary') return { process_id: 'synthetic' };
    throw new Error(`Unexpected command: ${command}`);
  });
  try {
    const { handleGenerateSummary, handleRegenerateSummary } = render();
    const results = Promise.all([handleGenerateSummary(), handleGenerateSummary(), handleRegenerateSummary()]);
    await flush();
    assert.deepEqual(commands.map(([command]) => command), ['get_ollama_models']);
    assert.equal(render().isCheckingModels, true);
    models.resolve([{ name: 'synthetic-model' }]);
    await results;
    assert.deepEqual(commands.map(([command]) => command), ['get_ollama_models', 'api_generate_meeting_summary']);
    assert.equal(render().isCheckingModels, false);
  } finally { unmount(); }
});

test('Built-in AI preflight reports a missing download and opens settings without generating', async () => {
  const commands = [];
  const messages = [];
  let opened = 0;
  const { render, unmount } = summaryGeneration('builtin-ai', async (command, args) => {
    commands.push(command);
    if (command === 'builtin_ai_is_model_ready') {
      assert.equal(args.refresh, true);
      return false;
    }
    if (command === 'builtin_ai_get_model_info') return { status: { type: 'not_downloaded' } };
    throw new Error(`Unexpected command: ${command}`);
  }, { info() {}, error: message => messages.push(message) }, () => { opened++; });
  try {
    await render().handleRegenerateSummary();
    assert.deepEqual(commands, ['builtin_ai_is_model_ready', 'builtin_ai_get_model_info']);
    assert.deepEqual(messages, ['Built-in AI model not downloaded']);
    assert.equal(opened, 1);
    assert.equal(render().isCheckingModels, false);
  } finally { unmount(); }
});

test('recovered notes keep readable text without activating unverified citations', () => {
  const { recoveredSummary } = loadTsModule('src/lib/recovered-summary.ts');
  const summary = recoveredSummary('## Decisions\n- Keep the new deadline. [00:01:00](#clawscribe-source-abc)\n- Raw href: #clawscribe-source-def');
  assert.match(summary.markdown, /Decisions/);
  assert.match(summary.markdown, /Keep the new deadline/);
  assert.doesNotMatch(summary.markdown, /#clawscribe-source-|\]\(/);
});

test('citation previews use the saved map and stale playback reveals the row first', async () => {
  const hooks = createHookHarness();
  const calls = [];
  const key = 'abc123';
  const reference = { transcript_id: 'row-1', transcript_index: 35, text: 'The current transcript now says this.', timestamp: null, stale: true };
  const { SummarySources } = loadTsModule('src/components/MeetingDetails/SummarySources.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      if (command === 'api_get_summary_sources') return [{ key, timestamp: 18 }];
      if (command === 'api_resolve_summary_source') {
        assert.equal(args.key, key);
        return reference;
      }
      assert.equal(command, 'api_get_meeting_transcripts');
      assert.equal(args.offset, 35);
      assert.equal(args.limit, 1);
      return { transcripts: [{ id: 'row-1', audio_start_time: 21.5 }] };
    } },
    '@/components/ui/button': { Button: 'Button' },
    '@/components/ui/dialog': dialogs,
    '@/components/ui/accordion': stubs('Accordion AccordionItem AccordionContent AccordionTrigger'),
  });
  const render = () => hooks.render(() => SummarySources({
    meetingId: 'test-meeting', revision: 'summary',
    onReveal: async source => { calls.push(['reveal', source.transcript_id]); },
    onPlay: async offset => { calls.push(['play', offset]); },
    onNavigate: () => calls.push('focus transcript'),
  }));
  try {
    render(); await flush();
    nodes(render()).find(node => node.type === 'Accordion').props.onValueChange('sources');
    render(); await flush();
    const preview = nodes(render()).find(node => node.type === 'Button' && text(node).includes(reference.text));
    assert.ok(preview);
    assert.match(text(preview), /Changed/);
    preview.props.onClick({ currentTarget: {} }); await flush();
    const dialog = nodes(render()).find(node => node.type === 'DialogContent');
    assert.match(text(dialog), /This passage has changed/);
    assert.equal(button(dialog, 'Show in transcript').props.disabled, false);
    assert.equal(button(dialog, 'Play from here').props.disabled, false);
    button(dialog, 'Play from here').props.onClick(); await flush();
    assert.equal(nodes(render()).find(node => node.type === 'Dialog').props.open, false);
    dialog.props.onCloseAutoFocus({ preventDefault() {} });
    assert.deepEqual(calls, [['reveal', 'row-1'], ['play', 21.5], 'focus transcript']);
  } finally { hooks.unmount(); }
});

function summaryPanel(t, initial = {}) {
  const hooks = createHookHarness();
  const copied = [];
  const highlights = new Map();
  let observation;
  const previousWindow = globalThis.window;
  globalThis.window = {
    CSS: { highlights },
    Highlight: class { constructor(...ranges) { this.ranges = ranges; } },
    MutationObserver: class {
      constructor(callback) { observation = callback; }
      observe() {}
      disconnect() { observation = undefined; }
    },
  };
  const { SummaryPanel } = loadTsModule('src/components/MeetingDetails/SummaryPanel.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    './MeetingChat': stubs('MeetingChat'), './SavedLiveAnswers': stubs('SavedLiveAnswers'), './FollowUpEmail': stubs('FollowUpEmail'),
    './SummarySources': stubs('SummarySources'), './TextReplaceDialog': stubs('TextReplaceDialog'), './ActionIcon': stubs('ActionIcon'),
    './SummaryGeneratorButtonGroup': stubs('SummaryGeneratorButtonGroup'), './SummaryUpdaterButtonGroup': stubs('SummaryUpdaterButtonGroup'),
    '@/components/AISummary/BlockNoteSummaryView': stubs('BlockNoteSummaryView'), '@/components/AISummary/SummarySurface': stubs('SummarySurface'),
    '@/components/LanguagePickerPopover': stubs('LanguagePickerPopover'), '@/hooks/useRecentLanguages': { useRecentLanguages: () => ({ addRecent() {} }) },
    '@/components/ui/input': stubs('Input'), '@/components/ui/button': stubs('Button'), '@/components/ui/textarea': stubs('Textarea'),
    '@/components/ui/tooltip': stubs('Tooltip TooltipTrigger TooltipContent'), '@/components/ui/popover': stubs('Popover PopoverTrigger PopoverContent'),
    '@/components/ui/dialog': dialogs, '@/components/ui/progress': stubs('Progress'),
    '@/lib/analytics': { __esModule: true, default: { trackButtonClick() {} } },
    '@/lib/summary-language-preferences': { readMeetingSummaryLanguage: async () => ({ language: null, storage: 'metadata' }) },
    '@/lib/summary-find': { collectSummaryRanges: (root, query) => root.ranges.filter(range => range.label.includes(query)) },
    sonner: { toast: { success() {}, error() {} } },
  });
  const clipboard = Object.getOwnPropertyDescriptor(globalThis.navigator, 'clipboard');
  Object.defineProperty(globalThis.navigator, 'clipboard', { configurable: true, value: { writeText: async value => { copied.push(value); } } });
  t.after(() => {
    if (clipboard) Object.defineProperty(globalThis.navigator, 'clipboard', clipboard);
    else delete globalThis.navigator.clipboard;
    hooks.unmount();
    globalThis.window = previousWindow;
  });
  let props = {
    details: { transcriptChanged: false }, meeting: { id: 'test-meeting', created_at: '2026-01-01' },
    aiSummary: { markdown: 'Saved notes' }, modelConfig: {}, summaryStatus: 'idle', transcripts: [{}], saveState: 'saved',
    onGenerateSummary: async () => {}, onSaveSummary: async () => {}, customPrompt: '', getSummaryStatusMessage: () => '',
    ...initial,
  };
  return { copied, highlights, mutation: () => observation?.(),
    set: next => { props = { ...props, ...next }; }, render: () => hooks.render(() => SummaryPanel(props)) };
}

test('summary Find opens from the editor, highlights as you type, wraps and clears', async t => {
  const panel = summaryPanel(t);
  const searchBox = () => nodes(panel.render()).find(node => node.props['aria-label'] === 'Find in summary' && node.type === 'Input');
  const root = panel.render();
  let focused = 0;
  let prevented = false;
  const editor = { isContentEditable: true };
  root.props.ref.current = { focus: () => { focused++; } };
  root.props.onKeyDownCapture({
    target: editor, currentTarget: { contains: target => target === editor }, ctrlKey: true,
    nativeEvent: { key: 'f', ctrlKey: true }, preventDefault: () => { prevented = true; }, stopPropagation() {},
  });
  assert.equal(prevented, true);
  assert.ok(searchBox());
  const first = { label: 'Budget first', getBoundingClientRect: () => ({ top: 200, bottom: 210, height: 10 }) };
  const second = { label: 'Budget second', getBoundingClientRect: () => ({ top: 220, bottom: 230, height: 10 }) };
  const scroll = { scrollTop: 0, getBoundingClientRect: () => ({ top: 0, bottom: 100, height: 100 }) };
  const tree = nodes(panel.render());
  tree.find(node => node.props.className?.includes('::highlight')).props.ref.current = { ranges: [first, second] };
  tree.find(node => node.props.className === 'min-h-0 min-w-0 flex-1 overflow-y-auto').props.ref.current = scroll;
  searchBox().props.onChange({ target: { value: 'Budget' } });
  panel.render();
  assert.match(text(panel.render()), /1 of 2/);
  assert.ok(scroll.scrollTop > 0);
  assert.equal(panel.highlights.get('clawscribe-summary-find').ranges.length, 2);
  assert.equal(panel.highlights.get('clawscribe-summary-find-current').ranges[0], first);
  searchBox().props.onKeyDown({ key: 'Enter', shiftKey: true, nativeEvent: {}, preventDefault() {} });
  assert.match(text(panel.render()), /2 of 2/);
  assert.equal(panel.highlights.get('clawscribe-summary-find-current').ranges[0], second);
  tree.find(node => node.props.className?.includes('::highlight')).props.ref.current.ranges = [first];
  panel.mutation(); panel.render();
  assert.match(text(panel.render()), /1 of 1/);
  searchBox().props.onChange({ target: { value: 'No keyword' } });
  panel.render();
  assert.match(text(panel.render()), /No matches/);
  searchBox().props.onKeyDown({ key: 'Escape', nativeEvent: {}, preventDefault() {}, stopPropagation() {} });
  assert.equal(searchBox(), undefined);
  assert.equal(panel.highlights.size, 0);
  assert.equal(focused, 1);
  await flush();
});

test('regeneration uses the stale notice and keeps progress outside the scrolling notes', async t => {
  const generated = [];
  const panel = summaryPanel(t, { details: { transcriptChanged: true }, customPrompt: 'Meeting context', onGenerateSummary: async prompt => { generated.push(prompt); } });
  const stale = panel.render();
  assert.match(text(stale), /Transcript changed since generation/);
  assert.equal(nodes(stale).find(node => node.type === 'SummaryGeneratorButtonGroup').props.showRegenerate, false);
  button(stale, 'Regenerate').props.onClick();
  assert.deepEqual(generated, ['Meeting context']);
  panel.set({ summaryStatus: 'regenerating', jobSnapshot: { status: 'regenerating', progress: { completed: 2, total: 4, level: 1 } } });
  const running = panel.render();
  assert.match(text(running), /Regenerating summary/);
  assert.equal(button(running, 'Regenerate'), undefined);
  const progress = nodes(running).find(node => node.type === 'Progress');
  assert.equal(progress.props.value, 50);
  const scroller = nodes(running).find(node => node.props.className === 'min-h-0 min-w-0 flex-1 overflow-y-auto');
  assert.equal(nodes(scroller).includes(progress), false);
  await flush();
});

test('recovered notes can be copied and save only after confirmation', async t => {
  const notes = 'The first excerpt finished.';
  const saves = [];
  const saving = deferred();
  const panel = summaryPanel(t, { jobSnapshot: { status: 'failed', partial_notes: notes }, onSaveSummary: summary => { saves.push(summary); return saving.promise; } });
  const recovery = () => nodes(panel.render()).find(node => node.type === 'section');
  const dialog = () => nodes(panel.render()).find(node => node.type === 'Dialog');
  button(recovery(), 'Copy').props.onClick(); await flush();
  assert.deepEqual(panel.copied, [notes]);
  button(recovery(), 'Use as summary').props.onClick();
  assert.equal(dialog().props.open, true);
  assert.deepEqual(saves, []);
  button(dialog(), 'Cancel').props.onClick();
  assert.equal(dialog().props.open, false);
  button(recovery(), 'Use as summary').props.onClick();
  const confirm = button(dialog(), 'Use as summary');
  confirm.props.onClick(); confirm.props.onClick();
  assert.equal(saves.length, 1);
  assert.match(saves[0].markdown, /The first excerpt finished/);
  assert.equal(button(dialog(), 'Saving…').props.disabled, true);
  saving.resolve(); await flush();
  assert.equal(dialog().props.open, false);
  assert.equal(button(recovery(), 'Use as summary'), undefined);
});

test('the meeting header overflow is the single recording-folder action', () => {
  const hooks = createHookHarness();
  const { MeetingHeader } = loadTsModule('src/components/MeetingDetails/MeetingHeader.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@tauri-apps/api/event': { listen: async () => () => {} },
    '@/components/EditableTitle': stubs('EditableTitle'), './ActionIcon': stubs('ActionIcon'), './ProjectTags': stubs('ProjectTags'),
    './MeetingExportButtons': stubs('MeetingExportButtons'), './ReviewedActions': stubs('ReviewedActions'), './FollowUpEmail': stubs('FollowUpEmail'),
    './SeriesPreparation': stubs('SavedSeriesReview'), '@/components/ui/dropdown-menu': dropdown, '@/components/ui/button': stubs('Button'),
    '@/services/meetingActionsService': { getMeetingActionsOverview: async () => null },
  });
  const { SummaryUpdaterButtonGroup } = loadTsModule('src/components/MeetingDetails/SummaryUpdaterButtonGroup.tsx', {
    'react/jsx-runtime': { jsx, jsxs: jsx }, '@/components/ui/dropdown-menu': dropdown, '@/lib/analytics': {},
  });
  let opened = 0;
  const render = folder_path => hooks.render(() => MeetingHeader({
    meeting: { id: 'test-meeting', created_at: '2026-01-01', folder_path }, getMarkdown: async () => '', onOpenFolder: async () => { opened++; },
  }));
  try {
    const folderItems = tree => nodes(tree).filter(node => node.type === 'DropdownMenuItem' && text(node) === 'Open recording folder');
    const withFolder = folderItems(render('recording'));
    assert.equal(withFolder.length, 1);
    withFolder[0].props.onSelect();
    assert.equal(opened, 1);
    assert.equal(folderItems(render(null)).length, 0);
    assert.equal(folderItems(SummaryUpdaterButtonGroup({})).length, 0);
  } finally { hooks.unmount(); }
});
