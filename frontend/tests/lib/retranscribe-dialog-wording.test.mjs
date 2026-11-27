import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

const stubs = names => Object.fromEntries(names.map(name => [name, name]));
function text(node) {
  if (Array.isArray(node)) return node.map(text).join('');
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  return node ? text(node.props?.children) : '';
}

function findButton(node, label) {
  if (Array.isArray(node)) return node.map(child => findButton(child, label)).find(Boolean);
  if (node?.type === 'button' && text(node).trim() === label) return node;
  return node?.props && findButton(node.props.children, label);
}

function findElement(node, type) {
  if (Array.isArray(node)) return node.map(child => findElement(child, type)).find(Boolean);
  if (node?.type === type) return node;
  return node?.props && findElement(node.props.children, type);
}

async function setup(hasTranscript, editStateFails = false, cancelAccepted = true, options = {}) {
  const hooks = createHookHarness();
  const calls = [];
  const listeners = new Map();
  const messages = [];
  const invoke = async (command, args) => {
    calls.push(command);
    if (command === 'start_retranscription_command') return options.start?.(args);
    if (command === 'import_cloud_destination') return options.preview?.(args.provider);
    if (editStateFails && command === 'api_get_transcript_edit_state') throw new Error('Synthetic edit state failure');
    if (command === 'cancel_retranscription_command') return cancelAccepted;
    return { has_edits: true };
  };
  const modelHelpers = loadTsModule('src/hooks/useTranscriptionModels.ts', {
    react: hooks.react,
    '@tauri-apps/api/core': { invoke },
    '@/hooks/useCloudTranscription': {},
  });
  const { RetranscribeDialog } = loadTsModule(fileURLToPath(new URL('../../src/components/MeetingDetails/RetranscribeDialog.tsx', import.meta.url)), {
    react: hooks.react,
    '@tauri-apps/api/core': { invoke },
    '@tauri-apps/api/event': { listen: async (event, callback) => {
      await options.listenersReady;
      listeners.set(event, callback);
      return () => listeners.delete(event);
    } },
    sonner: { toast: { success: message => messages.push(message), info() {}, error() {} } },
    '@/contexts/ConfigContext': { useTranscriptionConfig: () => ({ selectedLanguage: options.language ?? 'auto', transcriptModelConfig: null }) },
    '@/hooks/useTranscriptionModels': {
      ...modelHelpers,
      useTranscriptionModels: () => ({
        availableModels: options.models ?? [{ provider: 'whisper', name: 'small', displayName: 'Whisper small', size_mb: 200 }],
        selectedModelKey: options.key ?? (options.models?.length === 0 ? '' : 'whisper:small'),
        loadingModels: options.loadingModels ?? false,
        fetchModels() {},
        resetSelection() {},
      }),
    },
    '@/lib/analytics': { track: async () => {}, trackError: async () => {} },
    'lucide-react': stubs(['RefreshCw', 'Globe', 'Loader2', 'AlertCircle', 'CheckCircle2', 'X', 'Cpu', 'Clock', 'Gauge', 'Zap', 'Hash']),
    '../ui/dialog': stubs(['Dialog', 'DialogContent', 'DialogDescription', 'DialogFooter', 'DialogHeader', 'DialogTitle']),
    '../ui/button': { Button: 'button' },
    '../ui/checkbox': { Checkbox: 'checkbox' },
    '../ui/progress': { Progress: 'progress' },
    'next/link': { default: 'a', __esModule: true },
    '../ui/select': stubs(['Select', 'SelectContent', 'SelectItem', 'SelectTrigger', 'SelectValue']),
  });
  const props = { open: true, hasTranscript, meetingId: 'synthetic-meeting', meetingFolderPath: 'synthetic-recording', onOpenChange() {} };
  const render = () => hooks.render(() => RetranscribeDialog(props));
  render();
  await flush();
  return { hooks, props, calls, listeners, messages, render };
}

test('first transcription offers creation without replacement or edit-state lookup', async () => {
  const view = await setup(false);
  const content = text(view.render());
  assert.match(content, /Transcribe meeting/);
  assert.match(content, /Create a transcript from the saved audio/);
  assert.match(content, /Start transcription/);
  assert.doesNotMatch(content, /replace|corrections/i);
  assert.deepEqual(view.calls, []);
  view.hooks.unmount();
});

test('existing transcript keeps replacement wording and correction warnings', async () => {
  const view = await setup(true);
  const content = text(view.render());
  assert.match(content, /Retranscribe meeting/);
  assert.match(content, /Replace the current transcript using the saved audio/);
  assert.match(content, /Replace transcript/);
  assert.match(content, /Retranscription replaces the current transcript\./);
  assert.match(content, /Your corrections will be replaced/);
  assert.deepEqual(view.calls, ['api_get_transcript_edit_state']);
  view.hooks.unmount();
});

test('edit-state warnings disappear when opening a meeting without a transcript', async () => {
  const view = await setup(true, true);
  assert.match(text(view.render()), /Could not check for corrections/);
  view.props.open = false;
  view.render();
  view.props.hasTranscript = false;
  view.props.open = true;
  view.render();
  assert.doesNotMatch(text(view.render()), /replace|corrections/i);
  assert.equal(view.calls.length, 1);
  view.hooks.unmount();
});

test('a saved transcript with pending folder files is shown as a successful transcription', async () => {
  const view = await setup(true);
  await findButton(view.render(), 'Replace transcript').props.onClick();
  await view.listeners.get('retranscription-complete')({ payload: {
    meeting_id: 'synthetic-meeting', segments_count: 3, duration_seconds: 10,
    language: 'de', file_warning: true, provider: 'parakeet', model: 'parakeet-test',
  } });
  const content = text(view.render());
  assert.match(content, /Retranscription complete/);
  assert.match(content, /recording-folder copy needs updating/);
  assert.match(content, /with Parakeet · parakeet-test/);
  assert.doesNotMatch(content, /Retranscription failed/);
  view.hooks.unmount();
});

test('cloud retranscription requires separate approval for each destination and submits the reviewed fingerprint', async () => {
  const starts = [];
  const options = {
    key: 'cloud-whisper:whisper-1',
    language: 'auto-translate',
    models: [
      { provider: 'cloud-whisper', name: 'whisper-1', displayName: 'Hosted Whisper', size_mb: 0 },
      { provider: 'mai-transcribe', name: 'azure-speech', displayName: 'Azure Speech', size_mb: 0 },
    ],
    preview: provider => ({ endpoint: `https://${provider}.example.com`, fingerprint: `${provider}-reviewed` }),
    start: args => { starts.push(args); },
  };
  const view = await setup(true, false, true, options);
  assert.match(text(view.render()), /https:\/\/cloud-whisper\.example\.com/);
  assert.doesNotMatch(text(view.render()), /\(0 MB\)|translate to English/);
  assert.equal(findButton(view.render(), 'Replace transcript').props.disabled, true);
  findElement(view.render(), 'checkbox').props.onCheckedChange(true);
  assert.equal(findButton(view.render(), 'Replace transcript').props.disabled, false);

  options.key = 'mai-transcribe:azure-speech';
  assert.equal(findButton(view.render(), 'Replace transcript').props.disabled, true);
  await flush();
  assert.match(text(view.render()), /https:\/\/mai-transcribe\.example\.com/);
  assert.equal(findButton(view.render(), 'Replace transcript').props.disabled, true);
  findElement(view.render(), 'checkbox').props.onCheckedChange(true);
  await findButton(view.render(), 'Replace transcript').props.onClick();
  assert.equal(starts[0].cloudDestination, 'mai-transcribe-reviewed');
  assert.equal(starts[0].language, null);
  view.listeners.get('retranscription-complete')({ payload: {
    meeting_id: 'synthetic-meeting', segments_count: 3, duration_seconds: 10,
    provider: 'localWhisper', model: 'small', fallback_reason: 'The cloud provider could not complete transcription.',
  } });
  const completed = text(view.render());
  assert.match(completed, /Retranscribed with Whisper · small/);
  assert.match(completed, /The cloud provider could not complete transcription/);
  view.hooks.unmount();
});

test('a busy start returns to an actionable error state', async () => {
  const view = await setup(false, false, true, { start() { throw new Error('Another transcription job is active'); } });
  await findButton(view.render(), 'Start transcription').props.onClick();
  const content = text(view.render());
  assert.match(content, /Another transcription job is active/);
  assert.doesNotMatch(content, /Processing audio/);
  assert.ok(findButton(view.render(), 'Try again'));
  view.hooks.unmount();
});

test('submission is disabled until listeners and available models are ready', async () => {
  const readiness = deferred();
  const view = await setup(false, false, true, { listenersReady: readiness.promise });
  assert.equal(findButton(view.render(), 'Start transcription').props.disabled, true);
  readiness.resolve();
  await flush();
  assert.equal(findButton(view.render(), 'Start transcription').props.disabled, false);
  view.hooks.unmount();

  for (const options of [{ loadingModels: true }, { models: [] }]) {
    const unavailable = await setup(false, false, true, options);
    assert.equal(findButton(unavailable.render(), 'Start transcription').props.disabled, true);
    unavailable.hooks.unmount();
  }
});

test('closing during retranscription keeps progress and never requests cancellation', async () => {
  const view = await setup(true);
  view.props.onOpenChange = open => { view.props.open = open; };
  await findButton(view.render(), 'Replace transcript').props.onClick();
  findButton(view.render(), 'Run in background').props.onClick();
  assert.equal(view.props.open, false);
  view.render();
  view.listeners.get('retranscription-progress')({ payload: {
    meeting_id: 'synthetic-meeting', stage: 'decoding', progress_percentage: 10, message: 'Decoding synthetic audio',
  } });
  view.props.open = true;
  assert.match(text(view.render()), /Decoding synthetic audio/);
  assert.ok(!view.calls.includes('cancel_retranscription_command'));
  view.hooks.unmount();
});

test('late cancellation keeps the save active until completion', async () => {
  const view = await setup(true, false, false);
  await findButton(view.render(), 'Replace transcript').props.onClick();
  await findButton(view.render(), 'Cancel').props.onClick();
  assert.doesNotMatch(text(view.render()), /Cancelling|Retranscription failed/);

  view.listeners.get('retranscription-progress')({ payload: {
    meeting_id: 'synthetic-meeting', stage: 'saving', progress_percentage: 90,
    message: 'Updating recording files...',
  } });
  assert.equal(findButton(view.render(), 'Cancel').props.disabled, true);
  await view.listeners.get('retranscription-complete')({ payload: {
    meeting_id: 'synthetic-meeting', segments_count: 3, duration_seconds: 10, language: 'de', provider: 'whisper', model: 'small',
  } });
  assert.match(text(view.render()), /Retranscription complete/);
  view.hooks.unmount();
});

for (const hasTranscript of [false, true]) {
  test(`${hasTranscript ? 'replacement' : 'first'} transcription uses the matching completion toast and title`, async () => {
    const view = await setup(hasTranscript);
    await findButton(view.render(), hasTranscript ? 'Replace transcript' : 'Start transcription').props.onClick();
    await view.listeners.get('retranscription-complete')({ payload: {
      meeting_id: 'synthetic-meeting', segments_count: 3, duration_seconds: 10, language: 'de', provider: 'whisper', model: 'small',
    } });
    const name = hasTranscript ? 'Retranscription' : 'Transcription';
    assert.equal(view.messages[0], `${name} complete! 3 segments created.`);
    assert.match(text(view.render()), new RegExp(`${name} complete`));
    view.hooks.unmount();
  });

  test(`${hasTranscript ? 'replacement' : 'first'} transcription uses the matching failure title`, async () => {
    const view = await setup(hasTranscript);
    await findButton(view.render(), hasTranscript ? 'Replace transcript' : 'Start transcription').props.onClick();
    await view.listeners.get('retranscription-error')({ payload: { meeting_id: 'synthetic-meeting', error: 'Synthetic failure' } });
    assert.match(text(view.render()), new RegExp(`${hasTranscript ? 'Retranscription' : 'Transcription'} failed`));
    view.hooks.unmount();
  });
}
