import assert from 'node:assert/strict';
import fs from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';
import ts from 'typescript';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness } from './hook-harness.mjs';

const sourcePath = name => fileURLToPath(new URL(`../../src/${name}.ts`, import.meta.url));
const { fetchAllMeetingTranscripts } = loadTsModule(sourcePath('lib/meetingTranscripts'));
const rows = Array.from({ length: 2500 }, (_, index) => ({
  id: `row-${index}`, text: `Synthetic line ${index}.`, timestamp: '00:00:00',
  audio_start_time: index * 2, audio_end_time: index * 2 + 1,
}));

function pages(data, failOffset = -1) {
  const calls = [];
  const invoke = async (command, args) => {
    assert.equal(command, 'api_get_meeting_transcripts');
    assert.equal(args.meetingId, 'synthetic-meeting');
    assert.ok(args.limit > 0 && args.limit <= 1000);
    calls.push(args);
    if (args.offset === failOffset) throw new Error('Synthetic page failure');
    return { transcripts: data.slice(args.offset, args.offset + args.limit), total_count: data.length, has_more: args.offset + args.limit < data.length };
  };
  return { invoke, calls };
}

test('2500 transcripts load in three bounded pages and retain their order', async () => {
  const view = pages(rows);
  const result = await fetchAllMeetingTranscripts(view.invoke, 'synthetic-meeting');
  assert.equal(result.length, 2500);
  assert.deepEqual([...result].map(row => row.id), rows.map(row => row.id));
  assert.deepEqual(view.calls.map(call => call.offset), [0, 1000, 2000]);
  assert.ok(view.calls.every(call => call.limit === 1000));
});

test('an empty meeting needs just one page', async () => {
  const view = pages([]);
  assert.equal((await fetchAllMeetingTranscripts(view.invoke, 'synthetic-meeting')).length, 0);
  assert.equal(view.calls.length, 1);
});

test('page two failure rejects without returning the first page', async () => {
  const view = pages(rows, 1000);
  await assert.rejects(fetchAllMeetingTranscripts(view.invoke, 'synthetic-meeting'), /Synthetic page failure/);
  assert.equal(view.calls.length, 2);
});

test('duplicate IDs retain their first occurrence and offsets advance by fetched rows', async () => {
  const offsets = [];
  const result = await fetchAllMeetingTranscripts(async (_command, { offset }) => {
    offsets.push(offset);
    return offset === 0
      ? { transcripts: [rows[0], rows[1]], has_more: true }
      : { transcripts: [rows[1], rows[2]], has_more: false };
  }, 'synthetic-meeting');
  assert.deepEqual([...result].map(row => row.id), ['row-0', 'row-1', 'row-2']);
  assert.deepEqual(offsets, [0, 2]);
});

test('an empty page claiming more rows fails instead of looping indefinitely', async () => {
  await assert.rejects(fetchAllMeetingTranscripts(async () => ({ transcripts: [], has_more: true }), 'synthetic-meeting'), /did not advance/);
});

function caller(kind, failOffset = -1) {
  const hooks = createHookHarness();
  const pageSource = pages(rows, failOffset);
  const errors = [], copied = [], generated = [];
  const invoke = async (command, args) => {
    if (command === 'api_get_meeting_transcripts') return pageSource.invoke(command, args);
    if (command === 'api_get_summary') return { data: {} };
    if (command === 'api_generate_meeting_summary') { generated.push(args); return { process_id: 'synthetic-process' }; }
    throw new Error(`Unexpected command: ${command}`);
  };
  const mocks = {
    react: hooks.react,
    '@tauri-apps/api/core': { invoke },
    '@tauri-apps/plugin-dialog': { confirm: async () => true },
    sonner: { toast: { error: message => errors.push(message), info() {}, success() {}, warning() {} } },
    '@/lib/analytics': { trackCopy: async () => {}, trackSummaryGenerationStarted: async () => {}, trackCustomPromptUsed: async () => {} },
    '@/lib/meetingTranscripts': { fetchAllMeetingTranscripts },
    '@/lib/meeting-summary': loadTsModule(sourcePath('lib/meeting-summary')),
    '@/lib/summary-job-status': loadTsModule(sourcePath('lib/summary-job-status')),
    '@/lib/transcriptFormatting': loadTsModule(sourcePath('lib/transcriptFormatting')),
    '@/lib/meetingDrafts': loadTsModule(sourcePath('lib/meetingDrafts'), { '@tauri-apps/api/core': { invoke } }),
    '@/lib/openExternal': {},
    '@/lib/utils': {},
    '@/lib/meetingContext': { getMeetingContext: async () => '' },
    // The real formatters; only the browser clipboard write is replaced.
    '@/lib/clipboard-format': { ...loadTsModule(sourcePath('lib/clipboard-format')), writeClipboard: async content => { copied.push(content.text); return 'text'; } },
    '@/lib/meetingCalendar': { getMeetingCalendar: () => null },
    '@/lib/summary-language-preferences': { readMeetingSummaryLanguage: async () => ({ language: 'de' }) },
  };
  const name = kind === 'copy' ? 'useCopyOperations' : 'useSummaryGeneration';
  // Supply the browser clipboard global while exercising the unchanged production hook.
  const exports = {};
  vm.runInNewContext(ts.transpileModule(fs.readFileSync(sourcePath(`hooks/meeting-details/${name}`), 'utf8'), {
    compilerOptions: { esModuleInterop: true, module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2020 },
  }).outputText, {
    exports,
    require: specifier => {
      assert.ok(Object.hasOwn(mocks, specifier), `Missing mock: ${specifier}`);
      return mocks[specifier];
    },
    navigator: { clipboard: { writeText: async value => { copied.push(value); } } },
    console: { log() {}, warn() {}, error() {} },
  });
  const value = hooks.render(() => exports[name]({
    subscribeToSummary: () => () => {},
    meeting: { id: 'synthetic-meeting', title: 'Synthetic meeting', created_at: '2026-01-01' },
    transcripts: rows.slice(0, 100), meetingTitle: 'Synthetic meeting', aiSummary: null, blockNoteSummaryRef: { current: null },
    modelConfig: { provider: 'synthetic-provider', model: 'synthetic-model' }, isModelConfigLoading: false,
    selectedTemplate: 'standard_meeting', updateMeetingTitle() {}, setAiSummary() {},
  }));
  const action = kind === 'copy' ? value.handleCopyTranscript : kind === 'regenerate' ? value.handleRegenerateSummary : value.handleGenerateSummary;
  return { hooks, action, errors, copied, generated, calls: pageSource.calls };
}

test('copy uses all 2500 saved lines instead of the loaded 100', async () => {
  const view = caller('copy');
  await view.action();
  assert.deepEqual(view.errors, []);
  const actual = [...view.copied[0].matchAll(/Synthetic line (\d+)\./g)].map(match => Number(match[1]));
  assert.deepEqual(actual, Array.from({ length: 2500 }, (_, index) => index));
  assert.equal(view.calls.length, 3);
  view.hooks.unmount();
});

test('copy stops with one fetch-error toast when page two fails', async () => {
  const view = caller('copy', 1000);
  await view.action();
  assert.deepEqual(view.errors, ['Failed to fetch transcripts for copying']);
  assert.equal(view.copied.length, 0);
  assert.equal(view.calls.length, 2);
  view.hooks.unmount();
});

for (const kind of ['generate', 'regenerate']) {
  test(kind + ' asks the core for the saved meeting by ID without copying transcript pages', async () => {
    const view = caller(kind);
    await view.action();
    assert.deepEqual(view.errors, []);
    assert.equal(view.generated.length, 1);
    assert.equal(view.generated[0].meetingId, 'synthetic-meeting');
    assert.equal(view.generated[0].summaryLanguage, 'de');
    assert.equal('text' in view.generated[0], false);
    assert.equal(view.calls.length, 0);
    view.hooks.unmount();
  });
}

test('two generation requests during preflight start one job', async () => {
  const view = caller('generate');
  await Promise.all([view.action(), view.action()]);
  assert.deepEqual(view.errors, []);
  assert.equal(view.generated.length, 1);
  view.hooks.unmount();
});
