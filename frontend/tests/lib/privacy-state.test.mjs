import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, flush } from './hook-harness.mjs';

const { derivePrivacy, classifySummaryProvider } = loadTsModule('src/lib/privacy.ts');
const ready = value => ({ status: 'ready', value });

const base = () => ({
  recording: ready({ saveFolder: 'C:\\Recordings', saveAudio: true }),
  transcription: ready({ provider: 'parakeet', baseUrl: null }),
  cloudTranscription: false,
  summary: ready({ provider: 'builtin-ai', ollamaEndpoint: null, customEndpoint: null }),
  autoSummary: false,
  automaticSummaryApproved: false,
  liveSharing: ready(false),
  microsoft: ready({ connected: false, calendar: false }),
  confluenceRest: false,
  calendars: ready({ files: 0, subscriptions: 0 }),
  assistant: ready(false),
  updateCheck: false,
  semanticSearch: ready(true),
  webhook: ready({ enabled: false, configured: false, destination: null }),
});
const derive = overrides => derivePrivacy({ ...base(), ...overrides });
const rowOf = (model, id) => model.rows.find(row => row.id === id);
const factOf = (model, id) => model.facts.find(fact => fact.id === id);

test('a fully local setup reports nothing leaving and no automatic data', () => {
  const model = derive({});
  assert.equal(factOf(model, 'audio').tone, 'neutral');
  assert.equal(factOf(model, 'text').value, 'Stays on this PC');
  assert.equal(factOf(model, 'automatic').value, 'None');
  assert.equal(rowOf(model, 'summary').when, 'Never leaves');
  assert.equal(rowOf(model, 'live').tone, 'off');
  assert.equal(rowOf(model, 'exports').tone, 'off');
  assert.equal(rowOf(model, 'assistant').tone, 'off');
  assert.equal(rowOf(model, 'recording').detail, 'C:\\Recordings');
  assert.equal(rowOf(model, 'recording').detailIsPath, true);
});

test('webhook privacy lists the masked destination, triggers and selected content', () => {
  const webhook = {
    enabled: true, configured: true, destination: 'https://hooks.example.com',
    onSummary: true, onRecording: true, includeSummary: true, includeTranscript: true,
  };
  const model = derive({ webhook: ready(webhook) });
  assert.equal(rowOf(model, 'webhook').destination, 'https://hooks.example.com');
  assert.equal(rowOf(model, 'webhook').tone, 'warning');
  assert.match(rowOf(model, 'webhook').when, /summary is ready or a recording or import is saved/);
  assert.match(rowOf(model, 'webhook').detail, /summary and action items, transcript text/);
  assert.equal(factOf(model, 'text').value, 'Leaves automatically');
  assert.match(factOf(model, 'automatic').detail, /hooks.example.com/);

  const manual = derive({ webhook: ready({ ...webhook, onSummary: false, onRecording: false }) });
  assert.equal(rowOf(manual, 'webhook').when, 'When you export');
  assert.equal(factOf(manual, 'text').value, 'Leaves when you ask');
  assert.equal(factOf(manual, 'automatic').value, 'None');
  assert.equal(factOf(derive({ webhook: { status: 'error' } }), 'automatic').value, 'Could not check');
});

test('cloud transcription with a cloud engine says audio leaves, with the warning tone', () => {
  const model = derive({ cloudTranscription: true, transcription: ready({ provider: 'mai-transcribe', baseUrl: null }) });
  assert.equal(factOf(model, 'audio').tone, 'warning');
  assert.equal(factOf(model, 'audio').value, 'Leaves this PC');
  assert.equal(rowOf(model, 'transcription').tone, 'warning');
  assert.equal(rowOf(model, 'transcription').destination, 'Azure Speech');
});

test('a cloud engine with the cloud switch off, or the switch on with a local engine, keeps audio local', () => {
  const off = derive({ cloudTranscription: false, transcription: ready({ provider: 'cloud-whisper', baseUrl: 'https://api.openai.com/v1' }) });
  assert.equal(factOf(off, 'audio').tone, 'neutral');
  assert.match(rowOf(off, 'transcription').detail, /cloud transcription is off/);
  const local = derive({ cloudTranscription: true, transcription: ready({ provider: 'parakeet', baseUrl: null }) });
  assert.equal(factOf(local, 'audio').tone, 'neutral');
  assert.equal(rowOf(local, 'transcription').tone, 'neutral');
});

test('a hosted Whisper server on this PC does not count as audio leaving', () => {
  const model = derive({ cloudTranscription: true, transcription: ready({ provider: 'cloud-whisper', baseUrl: 'http://localhost:8080/v1' }) });
  assert.equal(factOf(model, 'audio').tone, 'neutral');
});

test('built-in and local Ollama summaries never leave; remote providers leave on action', () => {
  for (const provider of [{ provider: 'builtin-ai' }, { provider: 'ollama', ollamaEndpoint: null }, { provider: 'ollama', ollamaEndpoint: 'http://127.0.0.1:11434' }]) {
    const model = derive({ summary: ready({ ollamaEndpoint: null, customEndpoint: null, ...provider }) });
    assert.equal(rowOf(model, 'summary').tone, 'neutral', provider.provider);
    assert.equal(factOf(model, 'text').tone, 'neutral');
  }
  const remote = derive({ summary: ready({ provider: 'claude-code', ollamaEndpoint: null, customEndpoint: null }) });
  assert.equal(rowOf(remote, 'summary').tone, 'active');
  assert.equal(rowOf(remote, 'summary').destination, 'Claude Code');
  assert.equal(factOf(remote, 'text').tone, 'active');
  assert.equal(factOf(remote, 'automatic').value, 'None');
});

test('external automatic notes require approval of the current destination', () => {
  const summary = ready({ provider: 'openai', ollamaEndpoint: null, customEndpoint: null });
  const unapproved = derive({ autoSummary: true, summary });
  assert.equal(rowOf(unapproved, 'summary').when, 'When you generate');
  assert.equal(factOf(unapproved, 'text').value, 'Leaves when you ask');
  assert.equal(factOf(unapproved, 'automatic').value, 'None');
  const model = derive({ autoSummary: true, automaticSummaryApproved: true, summary });
  assert.equal(rowOf(model, 'summary').tone, 'warning');
  assert.equal(rowOf(model, 'summary').when, 'When a recording ends');
  assert.equal(factOf(model, 'automatic').value, 'Transcript text');
  assert.equal(factOf(model, 'automatic').tone, 'warning');
  assert.equal(factOf(model, 'text').tone, 'warning');
  const local = derive({ autoSummary: true });
  assert.equal(factOf(local, 'automatic').value, 'None');
});

test('Privacy uses the shared recording mode and the current automatic destination approval', async t => {
  const hooks = createHookHarness();
  const previous = { window: globalThis.window, document: globalThis.document };
  globalThis.window = new EventTarget();
  globalThis.document = new EventTarget();
  t.after(() => { hooks.unmount(); Object.assign(globalThis, previous); });
  const modelConfig = { provider: 'openai', model: 'fixture' };
  let recordingMode = 'audio_only';
  let approved = false;
  const { usePrivacyInput } = loadTsModule('src/components/settings/usePrivacyInput', {
    react: hooks.react,
    '@tauri-apps/api/core': { invoke: async command => ({
      get_recording_preferences: { auto_save: false, save_folder: 'Recordings' },
      api_get_model_config: modelConfig,
      list_ical_sources: [],
      get_mcp_access: {},
    })[command] },
    '@/contexts/ConfigContext': { useConfig: () => ({ isAutoSummary: true, modelConfig }) },
    '@/contexts/RecordingStateContext': { useRecordingState: () => ({ recordingMode }) },
    '@/hooks/useSummaryDestination': { useSummaryDestination: (_active, config) => ({ destination: { provider: config.provider, approved } }) },
    '@/hooks/useCloudTranscription': { useCloudTranscription: () => ({ loaded: true, enabled: false }) },
    '@/hooks/useWebhookSettings': { useWebhookSettings: () => base().webhook },
    '@/lib/exportDestinations': { getExportDestinations: () => ({}) },
    '@/services/knowledgeService': { knowledgeService: { liveSharing: async () => false, modelStatus: async () => ({ enabled: true }) } },
    '@/services/microsoftExportService': { microsoftExportService: { connectionStatus: async () => ({ state: 'disconnected' }) } },
  });
  const render = () => derivePrivacy(hooks.render(usePrivacyInput));
  render(); await flush();
  assert.equal(rowOf(render(), 'recording').detail, 'Recordings');
  assert.equal(factOf(render(), 'text').value, 'Leaves when you ask');
  recordingMode = 'live';
  approved = true;
  assert.equal(rowOf(render(), 'recording').detail, 'Audio is not saved');
  assert.equal(factOf(render(), 'text').value, 'Leaves automatically');
});

test('providers are classified from their endpoint where the provider alone cannot tell', () => {
  assert.equal(classifySummaryProvider('ollama', { ollamaEndpoint: 'http://nas.example.com:11434' }).kind, 'remote');
  assert.equal(classifySummaryProvider('custom-openai', { customEndpoint: 'http://localhost:1234/v1' }).kind, 'local');
  assert.equal(classifySummaryProvider('custom-openai', { customEndpoint: 'https://api.openai.com/v1' }).kind, 'remote');
  assert.equal(classifySummaryProvider('custom-openai', {}).kind, 'unknown');
  assert.equal(classifySummaryProvider('unknown-provider').kind, 'unknown');
  assert.equal(classifySummaryProvider('something-new').kind, 'unknown');
  assert.equal(classifySummaryProvider(null).kind, 'none');
  const unknown = derive({ summary: ready({ provider: 'unknown-provider', ollamaEndpoint: null, customEndpoint: null }) });
  assert.equal(rowOf(unknown, 'summary').destination, 'Depends on the provider');
});

test('live assistance follows the sharing switch and the provider', () => {
  assert.equal(rowOf(derive({ liveSharing: ready(false) }), 'live').tone, 'off');
  const remote = derive({ liveSharing: ready(true), summary: ready({ provider: 'claude', ollamaEndpoint: null, customEndpoint: null }) });
  assert.equal(rowOf(remote, 'live').when, 'When you ask');
  assert.equal(rowOf(remote, 'live').destination, 'Anthropic');
  const local = derive({ liveSharing: ready(true) });
  assert.equal(rowOf(local, 'live').tone, 'neutral');
});

test('exports, calendars, assistant access and the update check follow their settings', () => {
  assert.equal(rowOf(derive({}), 'exports').destination, 'Not connected');
  const connected = derive({ microsoft: ready({ connected: true, calendar: true }), calendars: ready({ files: 1, subscriptions: 2 }) });
  assert.equal(rowOf(connected, 'exports').tone, 'active');
  assert.equal(rowOf(connected, 'exports').destination, 'Microsoft 365');
  assert.equal(rowOf(connected, 'calendars').destination, 'Microsoft 365, 2 subscriptions, 1 file');
  assert.equal(rowOf(connected, 'calendars').when, 'Download only');
  assert.equal(rowOf(derive({ calendars: ready({ files: 1, subscriptions: 0 }) }), 'calendars').when, 'Never leaves');
  assert.equal(rowOf(derive({}), 'calendars').tone, 'off');
  assert.equal(rowOf(derive({ assistant: ready(true) }), 'assistant').destination, 'Local apps on this PC');
  assert.equal(rowOf(derive({ updateCheck: true }), 'update').when, 'At startup and periodically');
  assert.equal(rowOf(derive({ updateCheck: true }), 'update').tone, 'warning');
  assert.equal(rowOf(derive({ updateCheck: false }), 'update').when, 'Only when you check');
  assert.equal(rowOf(derive({ confluenceRest: true }), 'exports').destination, 'Confluence');
  assert.equal(rowOf(derive({ confluenceRest: true }), 'exports').tab, 'confluence');
});

test('conversation metrics stay local even when live assistance shares transcript text', () => {
  const model = derive({
    liveSharing: ready(true),
    summary: ready({ provider: 'anthropic' }),
  });
  assert.equal(rowOf(model, 'conversation').destination, 'This PC');
  assert.equal(rowOf(model, 'conversation').when, 'Never leaves');
});

test('every row links to the settings page that controls it', () => {
  const tabs = Object.fromEntries(derive({}).rows.map(row => [row.id, row.tab]));
  assert.deepEqual(tabs, {
    recording: 'recording', transcription: 'transcription', conversation: 'recording', memory: 'memory', summary: 'summary', live: 'summary',
    exports: 'ms365', webhook: 'webhook', calendars: 'calendars', assistant: 'assistant', update: 'general',
  });
});

test('loading and unreadable state is shown as such, never guessed', () => {
  const loading = derive({ summary: { status: 'loading' }, assistant: { status: 'error' }, recording: { status: 'error' } });
  assert.equal(rowOf(loading, 'summary').state, 'loading');
  assert.equal(rowOf(loading, 'summary').destination, 'Checking…');
  assert.equal(factOf(loading, 'text').state, 'loading');
  assert.equal(rowOf(loading, 'assistant').state, 'unavailable');
  assert.equal(rowOf(loading, 'assistant').destination, 'Could not check');
  assert.equal(rowOf(loading, 'recording').destination, 'Could not check');
  const audio = derive({ transcription: { status: 'error' } });
  assert.equal(factOf(audio, 'audio').value, 'Could not check');
  assert.equal(factOf(audio, 'audio').tone, 'off');
  const autoOnly = derive({ autoSummary: true, summary: { status: 'error' } });
  assert.equal(factOf(autoOnly, 'automatic').value, 'Could not check');
  // Live sharing off does not need the provider.
  assert.equal(rowOf(derive({ summary: { status: 'error' } }), 'live').state, 'ready');
});

test('built-in provider aliases count as local', () => {
  for (const alias of ['local-llama', 'localllama']) assert.equal(classifySummaryProvider(alias).kind, 'local');
});
