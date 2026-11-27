import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const noop = () => {};

// The summary listener and the meeting page hook wired together as in the app.
function setup(t) {
  const listeners = new Map();

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
    throw new Error(`Unexpected command ${command} ${JSON.stringify(args)}`);
  };
  const sonner = { toast: {
    info: noop, warning: noop, error: title => errors.push(title),
    success: (title, options) => toasts.push({ title, id: options?.id }),
  } };

  const { createSummaryUpdates } = loadTsModule('src/lib/summary-updates.ts', {
    '@tauri-apps/api/core': { invoke },
    '@tauri-apps/api/event': { listen: async (event, handler) => {
      listeners.set(event, handler);
      return () => listeners.delete(event);
    } },
  });
  const updates = createSummaryUpdates('saved-meeting');

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
  };
  const { useSummaryGeneration } = loadTsModule('src/hooks/meeting-details/useSummaryGeneration.ts', mocks);
  const page = () => pageHooks.render(() => useSummaryGeneration({
    subscribeToSummary: updates.subscribe,
    meeting: { id: 'saved-meeting', created_at: '2026-01-01' }, transcripts: [],
    modelConfig: { provider: 'synthetic', model: 'synthetic' }, isModelConfigLoading: false, selectedTemplate: '',
    updateMeetingTitle: noop, setAiSummary: value => aiSummary.push(value),
  }));
  t.after(() => pageHooks.unmount());

  const updateNativeStatus = (status, event = 'summary-job-changed') => {
    summaryReply.current = status;
    listeners.get(event)?.({ payload: 'saved-meeting' });
  };
  return { listeners, toasts, errors, aiSummary, page, pageHooks, updateNativeStatus };
}

test('a native summary stays active when the page leaves and is reattached on return', async t => {
  const app = setup(t);
  app.page().attachToRunningSummary(false);
  assert.equal(app.page().summaryStatus, 'processing');
  assert.ok(app.listeners.has('summary-job-changed'));
  app.pageHooks.reset();
  await flush();
  assert.equal(app.listeners.size, 0);
  assert.deepEqual(app.aiSummary, []);

  app.updateNativeStatus({ status: 'processing', progress: { completed: 2, total: 3, level: 1 } });
  app.page().attachToRunningSummary(false);
  await flush();
  assert.equal(app.page().jobUpdate.progress.completed, 2);
  app.updateNativeStatus({ status: 'processing', progress: { completed: 3, total: 3, level: 1 } }, 'summary-job-progress');
  await flush();
  assert.equal(app.page().jobUpdate.progress.completed, 3);
  app.updateNativeStatus({ status: 'completed', data: { markdown: 'Notes' } });
  await flush();
  assert.equal(app.page().summaryStatus, 'completed');
  assert.equal(app.aiSummary.at(-1).markdown, 'Notes');
  assert.equal(app.listeners.size, 0);
});

test('a failed native run shows the excerpts that survived', async t => {
  const app = setup(t);
  app.page().attachToRunningSummary(false);
  app.updateNativeStatus({ status: 'failed', error: 'Synthetic failure', partial_notes: 'Excerpt one completed.' });
  await flush();
  assert.equal(app.page().summaryStatus, 'error');
  assert.equal(app.page().jobUpdate.partial_notes, 'Excerpt one completed.');
  assert.equal(app.listeners.size, 0);
});
