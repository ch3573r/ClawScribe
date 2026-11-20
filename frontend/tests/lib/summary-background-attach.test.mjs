import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import ts from 'typescript';
import { createHookHarness } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const noop = () => {};

// The real polling hook, the real automatic-summary starter and the real meeting
// page hook, wired together as the app does.
function setup(t) {
  const timers = new Map();
  let nextTimer = 0;
  t.mock.method(globalThis, 'setInterval', callback => { const id = ++nextTimer; timers.set(id, callback); return id; });
  t.mock.method(globalThis, 'clearInterval', id => timers.delete(id));

  const toasts = [];
  const errors = [];
  const refreshed = [];
  const aiSummary = [];
  const summaryReply = { current: { status: 'processing' } };
  const invoke = async (command, args) => {
    if (command === 'api_get_summary') return summaryReply.current;
    if (command === 'get_recording_outcome') return null;
    if (command === 'api_get_model_config') return { model: 'synthetic-model' };
    if (command === 'api_get_default_template') return 'standard_meeting';
    if (command === 'api_process_transcript') return { process_id: 'synthetic-process' };
    throw new Error(`Unexpected command ${command} ${JSON.stringify(args)}`);
  };
  const sonner = { toast: {
    info: noop, warning: noop, error: title => errors.push(title),
    success: (title, options) => toasts.push({ title, id: options?.id }),
  } };

  const pollHooks = createHookHarness();
  const { useSummaryPolling } = loadTsModule('src/hooks/useSummaryPolling.ts', {
    react: pollHooks.react,
    '@tauri-apps/api/core': { invoke },
  });
  const polling = () => pollHooks.render(useSummaryPolling);
  polling();

  const { startAutomaticSummary } = loadTsModule('src/lib/auto-summary.ts', {
    '@tauri-apps/api/core': { invoke },
    sonner,
    '@/lib/recording-outcome': loadTsModule('src/lib/recording-outcome.ts'),
    '@/lib/meetingTranscripts': { fetchAllMeetingTranscripts: async () => [{ id: 'one', text: 'Synthetic line.' }] },
    '@/lib/transcriptFormatting': { formatTranscriptLine: line => line.text },
    '@/lib/meetingContext': { getMeetingContext: async () => '' },
    '@/lib/meetingCalendar': { getMeetingCalendar: () => null, attendeeChecklist: () => [] },
    '@/lib/sourceAttribution': { getSourceAttribution: () => false },
    '@/lib/speaker-attribution': { shouldShowSpeakerAttribution: () => false },
    '@/lib/summary-language-preferences': {
      readMeetingSummaryLanguage: async () => ({ language: 'en' }),
      readCachedDetectedSummaryLanguage: async () => null,
      detectAndCacheSummaryLanguage: async () => ({ language: 'en' }),
    },
  });

  const pageHooks = createHookHarness();
  const mocks = {
    react: pageHooks.react,
    '@tauri-apps/api/core': { invoke },
    '@tauri-apps/plugin-dialog': { confirm: async () => true },
    sonner,
    '@/lib/analytics': { trackSummaryGenerationCompleted: async () => {}, trackSummaryGenerationStarted: async () => {}, trackCustomPromptUsed: async () => {} },
    '@/lib/meetingTranscripts': { fetchAllMeetingTranscripts: async () => [] },
    '@/lib/transcriptFormatting': { formatTranscriptLine: () => '' },
    '@/lib/meetingDrafts': { confirmSummaryRegeneration: async () => true },
    '@/lib/openExternal': {},
    '@/lib/utils': {},
    '@/lib/meetingContext': { getMeetingContext: async () => '' },
    '@/lib/meetingCalendar': { getMeetingCalendar: () => null },
    '@/lib/summary-language-preferences': {},
    // The sidebar's polling, as the provider hands it to the page.
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => {
      const api = polling();
      return { startSummaryPolling: api.startSummaryPolling, stopSummaryPolling: api.stopSummaryPolling, attachSummaryPolling: api.attachSummaryPolling };
    } },
  };
  const exports = {};
  const source = fs.readFileSync(new URL('../../src/hooks/meeting-details/useSummaryGeneration.ts', import.meta.url), 'utf8');
  vm.runInNewContext(ts.transpileModule(source, { compilerOptions: { esModuleInterop: true, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 } }).outputText, {
    exports, console: { log() {}, warn() {}, error() {} },
    require: specifier => { assert.ok(Object.hasOwn(mocks, specifier), `Missing mock: ${specifier}`); return mocks[specifier]; },
  });
  const page = () => pageHooks.render(() => exports.useSummaryGeneration({
    meeting: { id: 'saved-meeting', created_at: '2026-01-01' }, transcripts: [],
    modelConfig: { provider: 'synthetic', model: 'synthetic' }, isModelConfigLoading: false, selectedTemplate: '',
    updateMeetingTitle: noop, setAiSummary: value => aiSummary.push(value),
  }));
  t.after(() => { pollHooks.unmount(); pageHooks.unmount(); });

  const startBackground = () => startAutomaticSummary({
    meetingId: 'saved-meeting',
    modelConfig: { provider: 'synthetic', model: 'synthetic-model' },
    startSummaryPolling: (...args) => polling().startSummaryPolling(...args),
    refetchMeetings: async () => { refreshed.push('refetched'); },
    openMeeting: noop,
  });
  const completed = { status: 'completed', data: { markdown: 'Notes' } };
  const finish = async () => { summaryReply.current = completed; await timers.get([...timers.keys()].at(-1))(); };
  const fail = async () => { summaryReply.current = { status: 'failed', error: 'Synthetic failure' }; await timers.get([...timers.keys()].at(-1))(); };
  return { timers, toasts, errors, refreshed, aiSummary, polling, page, pageHooks, startBackground, finish, fail };
}

test('a background summary survives the meeting page attaching and leaving, and still finishes', async t => {
  const app = setup(t);
  assert.equal(await app.startBackground(), true);
  assert.equal(app.timers.size, 1, 'the background poll runs');

  app.page().attachToRunningSummary(false);
  assert.equal(app.page().summaryStatus, 'processing', 'the page shows it as generating');
  assert.equal(app.timers.size, 1, 'attaching joined the poll instead of replacing it');

  // The page lets go: its own cleanup, then the page route's cleanup.
  app.pageHooks.unmount();
  app.polling().stopSummaryPolling('saved-meeting', { keepBackground: true });
  assert.equal(app.timers.size, 1, 'the poll carries on');

  await app.finish();
  assert.deepEqual(app.toasts.map(toast => toast.title), ['Summary ready'], 'the starter still announces it');
  assert.deepEqual(app.refreshed, ['refetched'], 'and refreshes the meeting list');
  assert.deepEqual(app.aiSummary, [], 'a page that has gone is not updated');
  assert.equal(app.timers.size, 0, 'the finished poll is gone');
});

test('while the page stays open both hear the result, and the toast is shared, not doubled', async t => {
  const app = setup(t);
  await app.startBackground();
  app.page().attachToRunningSummary(false);
  await app.finish();
  assert.deepEqual(JSON.parse(JSON.stringify(app.aiSummary)), [{ markdown: 'Notes' }], 'the open page shows the notes');
  assert.equal(app.page().summaryStatus, 'completed');
  assert.deepEqual(app.refreshed, ['refetched']);
  assert.ok(app.toasts.length >= 1);
  assert.equal(new Set(app.toasts.map(toast => toast.id)).size, 1, 'every "Summary ready" uses one toast id, so it appears once');
  assert.equal(app.toasts[0].id, 'summary-ready:saved-meeting');
});

test('a poll the page attached to on its own stops when the page leaves', async t => {
  const app = setup(t);
  app.page().attachToRunningSummary(false);
  assert.equal(app.timers.size, 1, 'no background poll existed, so the page started one');
  app.pageHooks.unmount();
  assert.equal(app.timers.size, 0, 'nobody listens, so it stops');
});

test('a failed background run is reported by its starter even after the page left', async t => {
  const app = setup(t);
  await app.startBackground();
  app.page().attachToRunningSummary(false);
  app.pageHooks.unmount();
  app.polling().stopSummaryPolling('saved-meeting', { keepBackground: true });
  await app.fail();
  assert.deepEqual(app.errors, ['Could not generate the summary']);
  assert.deepEqual(app.aiSummary, []);
  assert.equal(app.timers.size, 0);
});
