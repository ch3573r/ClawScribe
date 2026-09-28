import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, flush } from './hook-harness.mjs';

const stubs = names => Object.fromEntries(names.map(name => [name, name]));
function text(node) {
  if (Array.isArray(node)) return node.map(text).join('');
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  return node ? text(node.props?.children) : '';
}

async function setup(hasTranscript, editStateFails = false) {
  const hooks = createHookHarness();
  const calls = [];
  const listeners = new Map();
  const messages = [];
  const { RetranscribeDialog } = loadTsModule(fileURLToPath(new URL('../../src/components/MeetingDetails/RetranscribeDialog.tsx', import.meta.url)), {
    react: hooks.react,
    '@tauri-apps/api/core': { invoke: async command => {
      calls.push(command);
      if (editStateFails) throw new Error('Synthetic edit state failure');
      return { has_edits: true };
    } },
    '@tauri-apps/api/event': { listen: async (event, callback) => { listeners.set(event, callback); return () => listeners.delete(event); } },
    sonner: { toast: { success: message => messages.push(message), info() {} } },
    '@/contexts/ConfigContext': { useConfig: () => ({ selectedLanguage: 'auto', transcriptModelConfig: null }) },
    '@/hooks/useTranscriptionModels': { useTranscriptionModels: () => ({ availableModels: [], fetchModels() {}, resetSelection() {} }) },
    '@/lib/analytics': { track: async () => {}, trackError: async () => {} },
    'lucide-react': stubs(['RefreshCw', 'Globe', 'Loader2', 'AlertCircle', 'CheckCircle2', 'X', 'Cpu', 'Clock', 'Gauge', 'Zap', 'Hash']),
    '../ui/dialog': stubs(['Dialog', 'DialogContent', 'DialogDescription', 'DialogFooter', 'DialogHeader', 'DialogTitle']),
    '../ui/button': { Button: 'button' },
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
  assert.match(content, /Transcribe Meeting/);
  assert.match(content, /Create a transcript from the saved audio/);
  assert.match(content, /Start Transcription/);
  assert.doesNotMatch(content, /replace|corrections/i);
  assert.deepEqual(view.calls, []);
  view.hooks.unmount();
});

test('existing transcript keeps replacement wording and correction warnings', async () => {
  const view = await setup(true);
  const content = text(view.render());
  assert.match(content, /Retranscribe Meeting/);
  assert.match(content, /Replace the current transcript using the saved audio/);
  assert.match(content, /Replace Transcript/);
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

for (const hasTranscript of [false, true]) {
  test(`${hasTranscript ? 'replacement' : 'first'} transcription uses the matching completion toast and title`, async () => {
    const view = await setup(hasTranscript);
    await view.listeners.get('retranscription-complete')({ payload: {
      meeting_id: 'synthetic-meeting', segments_count: 3, duration_seconds: 10, language: 'de',
    } });
    const name = hasTranscript ? 'Retranscription' : 'Transcription';
    assert.equal(view.messages[0], `${name} complete! 3 segments created.`);
    assert.match(text(view.render()), new RegExp(`${name} Complete`));
    view.hooks.unmount();
  });

  test(`${hasTranscript ? 'replacement' : 'first'} transcription uses the matching failure title`, async () => {
    const view = await setup(hasTranscript);
    await view.listeners.get('retranscription-error')({ payload: { meeting_id: 'synthetic-meeting', error: 'Synthetic failure' } });
    assert.match(text(view.render()), new RegExp(`${hasTranscript ? 'Retranscription' : 'Transcription'} Failed`));
    view.hooks.unmount();
  });
}
