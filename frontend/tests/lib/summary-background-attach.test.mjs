import assert from 'node:assert/strict';
import test from 'node:test';
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
  const { useSummaryGeneration } = loadTsModule('src/hooks/meeting-details/useSummaryGeneration.ts', mocks);
  const page = () => pageHooks.render(() => useSummaryGeneration({
    meeting: { id: 'saved-meeting', created_at: '2026-01-01' }, transcripts: [],
    modelConfig: { provider: 'synthetic', model: 'synthetic' }, isModelConfigLoading: false, selectedTemplate: '',
    updateMeetingTitle: noop, setAiSummary: value => aiSummary.push(value),
  }));
  t.after(() => { pollHooks.unmount(); pageHooks.unmount(); });

  const updateNativeStatus = status => { summaryReply.current = status; };
  const tick = async () => { await timers.values().next().value?.(); };
  return { timers, toasts, errors, aiSummary, page, pageHooks, updateNativeStatus, tick };
}

test('a native summary stays active when the page leaves and is reattached on return', async t => {
  const app = setup(t);
  app.page().attachToRunningSummary(false);
  assert.equal(app.page().summaryStatus, 'processing');
  assert.equal(app.timers.size, 1);
  app.pageHooks.unmount();
  assert.equal(app.timers.size, 0);
  assert.deepEqual(app.aiSummary, []);

  app.updateNativeStatus({ status: 'processing', progress: { completed: 2, total: 3, level: 1 } });
  app.page().attachToRunningSummary(false);
  await app.tick();
  assert.equal(app.page().jobUpdate.progress.completed, 2);
  app.updateNativeStatus({ status: 'completed', data: { markdown: 'Notes' } });
  await app.tick();
  assert.equal(app.page().summaryStatus, 'completed');
  assert.equal(app.aiSummary.at(-1).markdown, 'Notes');
  assert.equal(app.timers.size, 0);
});

test('a failed native run shows the excerpts that survived', async t => {
  const app = setup(t);
  app.page().attachToRunningSummary(false);
  app.updateNativeStatus({ status: 'failed', error: 'Synthetic failure', partial_notes: 'Excerpt one completed.' });
  await app.tick();
  assert.equal(app.page().summaryStatus, 'error');
  assert.equal(app.page().jobUpdate.partial_notes, 'Excerpt one completed.');
  assert.equal(app.timers.size, 0);
});
