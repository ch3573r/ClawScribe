import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import ts from 'typescript';
import { createHookHarness } from './hook-harness.mjs';

// A summary already running in the core (for example the automatic one started
// while the user was elsewhere) is shown as generating and its result is taken.
function setup() {
  const hooks = createHookHarness();
  const polls = [];
  const events = { aiSummary: [], toasts: [], stopped: [] };
  const noop = () => {};
  const mocks = {
    react: hooks.react,
    '@tauri-apps/api/core': { invoke: async () => ({}) },
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
    '@/lib/summary-language-preferences': {},
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({
      startSummaryPolling: (meetingId, processId, onUpdate) => polls.push({ meetingId, onUpdate }),
      attachSummaryPolling: (meetingId, onUpdate) => { polls.push({ meetingId, onUpdate }); return () => {}; },
      stopSummaryPolling: id => events.stopped.push(id),
    }) },
  };
  const exports = {};
  const source = fs.readFileSync(new URL('../../src/hooks/meeting-details/useSummaryGeneration.ts', import.meta.url), 'utf8');
  vm.runInNewContext(ts.transpileModule(source, { compilerOptions: { esModuleInterop: true, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 } }).outputText, {
    exports, console: { log() {}, warn() {}, error() {} },
    require: specifier => { assert.ok(Object.hasOwn(mocks, specifier), `Missing mock: ${specifier}`); return mocks[specifier]; },
  });
  const render = () => hooks.render(() => exports.useSummaryGeneration({
    meeting: { id: 'saved-meeting', created_at: '2026-01-01' }, transcripts: [],
    modelConfig: { provider: 'synthetic', model: 'synthetic' }, isModelConfigLoading: false, selectedTemplate: '',
    updateMeetingTitle: noop, setAiSummary: value => events.aiSummary.push(value),
  }));
  return { render, polls, events, hooks };
}

test('attaching shows the run as generating and polls for the meeting once', t => {
  const app = setup();
  t.after(() => app.hooks.unmount());
  assert.equal(app.render().summaryStatus, 'idle');
  app.render().attachToRunningSummary(false);
  assert.equal(app.render().summaryStatus, 'processing');
  assert.equal(app.polls.length, 1);
  assert.equal(app.polls[0].meetingId, 'saved-meeting');
});

test('attaching to a regeneration says so', t => {
  const app = setup();
  t.after(() => app.hooks.unmount());
  app.render().attachToRunningSummary(true);
  assert.equal(app.render().summaryStatus, 'regenerating');
});

test('the result of an attached run is taken when it finishes', async t => {
  const app = setup();
  t.after(() => app.hooks.unmount());
  app.render().attachToRunningSummary(false);
  await app.polls[0].onUpdate({ status: 'completed', data: { markdown: 'Notes', MeetingName: '' } });
  assert.deepEqual(JSON.parse(JSON.stringify(app.events.aiSummary)), [{ markdown: 'Notes' }]);
  assert.equal(app.render().summaryStatus, 'completed');
  assert.deepEqual(app.events.toasts, ['Summary ready']);
});

test('a failed attached run ends in the error state', async t => {
  const app = setup();
  t.after(() => app.hooks.unmount());
  app.render().attachToRunningSummary(false);
  await app.polls[0].onUpdate({ status: 'failed', error: 'Synthetic failure' });
  const view = app.render();
  assert.equal(view.summaryStatus, 'error');
  assert.equal(view.summaryError, 'Synthetic failure');
});
