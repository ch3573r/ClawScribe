import assert from 'node:assert/strict';
import { loadTsModule } from './load-ts-module.mjs';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

// A summary already running in the core (for example the automatic one started
// while the user was elsewhere) is shown as generating and its result is taken.
function setup(start) {
  const hooks = createHookHarness();
  const subscriptions = [];
  const events = { aiSummary: [], toasts: [], invocations: [] };
  const noop = () => {};
  const mocks = {
    react: hooks.react,
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      events.invocations.push({ command, args });
      if (start && command === 'api_generate_meeting_summary') return start.promise;
      return { process_id: 'saved-meeting' };
    } },
    '@tauri-apps/plugin-dialog': { confirm: async () => true },
    sonner: { toast: { error: noop, info: noop, warning: noop, success: title => events.toasts.push(title) } },
    '@/lib/analytics': { trackSummaryGenerationCompleted: async () => {}, trackSummaryGenerationStarted: async () => {}, trackCustomPromptUsed: async () => {} },
    '@/lib/meetingTranscripts': { fetchAllMeetingTranscripts: async () => [] },
    '@/lib/transcriptFormatting': { formatTranscriptLine: () => '' },
    '@/lib/meetingDrafts': { confirmSummaryRegeneration: async () => true },
    '@/lib/openExternal': {},
    '@/lib/utils': {},
    '@/lib/meetingContext': { getMeetingContext: async () => '' },
    '@/lib/meetingCalendar': { getMeetingCalendar: () => null },
    '@/lib/summary-language-preferences': { readMeetingSummaryLanguage: async () => ({ language: null }) },
  };
  const { useSummaryGeneration } = loadTsModule('src/hooks/meeting-details/useSummaryGeneration.ts', mocks);
  const render = () => hooks.render(() => useSummaryGeneration({
    subscribeToSummary: (onUpdate, options) => {
      const subscription = { onUpdate, options, active: true };
      subscriptions.push(subscription);
      return () => { subscription.active = false; };
    },
    meeting: { id: 'saved-meeting', created_at: '2026-01-01' }, transcripts: [],
    modelConfig: { provider: 'synthetic', model: 'synthetic' }, isModelConfigLoading: false, selectedTemplate: '',
    updateMeetingTitle: noop, setAiSummary: value => events.aiSummary.push(value),
  }));
  return { render, subscriptions, events, hooks };
}

test('a start response arriving after unmount cannot subscribe to summary updates', async () => {
  const start = deferred();
  const app = setup(start);
  const generating = app.render().handleGenerateSummary('Project notes');
  await flush();
  assert.equal(app.events.invocations.at(-1)?.command, 'api_generate_meeting_summary');
  app.hooks.unmount();
  start.resolve({ process_id: 'saved-meeting' });
  await generating;
  assert.equal(app.subscriptions.length, 0);
});

test('attaching shows the run as generating and subscribes once', t => {
  const app = setup();
  t.after(() => app.hooks.unmount());
  assert.equal(app.render().summaryStatus, 'idle');
  app.render().attachToRunningSummary(false);
  assert.equal(app.render().summaryStatus, 'processing');
  assert.equal(app.subscriptions.length, 1);
  assert.equal(app.subscriptions[0].options.refresh, false);
});

test('attaching to a regeneration says so', t => {
  const app = setup();
  t.after(() => app.hooks.unmount());
  app.render().attachToRunningSummary(true);
  assert.equal(app.render().summaryStatus, 'regenerating');
});

test('translation retry keeps the saved draft and submits the explicit target once', async t => {
  const app = setup();
  t.after(() => app.hooks.unmount());
  await app.render().handleRetryTranslation('de', 'Project notes');
  assert.equal(app.render().summaryStatus, 'regenerating');
  assert.deepEqual(app.events.aiSummary, []);
  assert.equal(app.events.invocations.length, 1);
  assert.equal(app.subscriptions[0].options.refresh, true);
  const { command, args } = app.events.invocations[0];
  assert.equal(command, 'api_generate_meeting_summary');
  assert.equal(args.summaryLanguage, 'de');
  assert.equal(args.retryTranslation, true);
  assert.equal(args.customPrompt, 'Project notes');
});

test('generating while attached leaves the active subscription intact', async t => {
  const app = setup();
  t.after(() => app.hooks.unmount());
  app.render().attachToRunningSummary(false);
  await app.render().handleGenerateSummary();
  assert.equal(app.subscriptions.length, 1);
  assert.equal(app.render().summaryStatus, 'processing');
  await app.subscriptions[0].onUpdate({ status: 'skipped' });
  assert.equal(app.subscriptions[0].active, false);
  assert.equal(app.render().summaryStatus, 'idle');
});

test('the result of an attached run is taken when it finishes', async t => {
  const app = setup();
  t.after(() => app.hooks.unmount());
  app.render().attachToRunningSummary(false);
  await app.subscriptions[0].onUpdate({ status: 'completed', data: { markdown: 'Notes', MeetingName: '' } });
  assert.deepEqual(JSON.parse(JSON.stringify(app.events.aiSummary)), [{ markdown: 'Notes' }]);
  assert.equal(app.render().summaryStatus, 'completed');
  assert.deepEqual(app.events.toasts, ['Summary ready']);
});

test('a failed attached run ends in the error state', async t => {
  const app = setup();
  t.after(() => app.hooks.unmount());
  app.render().attachToRunningSummary(false);
  await app.subscriptions[0].onUpdate({ status: 'failed', error: 'Synthetic failure' });
  const view = app.render();
  assert.equal(view.summaryStatus, 'error');
  assert.equal(view.summaryError, 'Synthetic failure');
});

for (const [status, error] of [
  ['idle', 'Summary generation is no longer active. Please generate the summary again.'],
  ['completed', 'Summary generation completed without saved content. Please try again.'],
]) {
  test(`an attached run reports an error for ${status} without saved content`, async t => {
    const app = setup();
    t.after(() => app.hooks.unmount());
    app.render().attachToRunningSummary(false);
    await app.subscriptions[0].onUpdate({ status });
    const view = app.render();
    assert.equal(view.summaryStatus, 'error');
    assert.equal(view.summaryError, error);
    assert.equal(app.subscriptions[0].active, false);
  });
}

test('an encoded BlockNote result keeps its edited blocks and Markdown fallback', async t => {
  const app = setup();
  t.after(() => app.hooks.unmount());
  app.render().attachToRunningSummary(true);
  const summary = {
    markdown: 'Fallback notes',
    summary_json: [{ id: 'paragraph-1', type: 'paragraph', props: {}, content: [], children: [] }],
  };
  await app.subscriptions[0].onUpdate({ status: 'completed', data: JSON.stringify(summary) });
  assert.deepEqual(JSON.parse(JSON.stringify(app.events.aiSummary)), [summary]);
  assert.equal(app.render().summaryStatus, 'completed');
});

test('legacy notes follow their saved order and ignore metadata', async t => {
  const app = setup();
  t.after(() => app.hooks.unmount());
  app.render().attachToRunningSummary(false);
  const data = {
    last: { title: 'Last', blocks: [{ id: 'stable', type: 'bullet', content: '  Done  ' }] },
    first: { title: 'First', blocks: [] },
    _section_order: ['first', 'last'],
    MeetingName: 'Review',
  };
  await app.subscriptions[0].onUpdate({ status: 'completed', data });
  const summary = app.events.aiSummary[0];
  assert.deepEqual(Object.keys(summary), ['first', 'last']);
  assert.deepEqual({ ...summary.last.blocks[0] }, { id: 'stable', type: 'bullet', content: 'Done', color: 'default' });
});

test('the legacy edit envelope retains repeated headings', async t => {
  const app = setup();
  t.after(() => app.hooks.unmount());
  app.render().attachToRunningSummary(true);
  const sections = ['First', 'Second'].map(content => ({ title: 'Topic', blocks: [{ content }] }));
  await app.subscriptions[0].onUpdate({ status: 'completed', data: { MeetingNotes: { sections } } });
  const saved = Object.values(app.events.aiSummary[0]);
  assert.deepEqual(saved.map(section => section.title), ['Topic', 'Topic']);
  assert.deepEqual(saved.map(section => section.blocks[0].content), ['First', 'Second']);
  assert.notEqual(saved[0].blocks[0].id, saved[1].blocks[0].id);
});
