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

function findButton(node, label) {
  if (Array.isArray(node)) return node.map(child => findButton(child, label)).find(Boolean);
  if (node?.type === 'button' && text(node).trim() === label) return node;
  return node?.props && findButton(node.props.children, label);
}

async function setup(hasTranscript, editStateFails = false, cancelAccepted = true) {
  const hooks = createHookHarness();
  const calls = [];
  const listeners = new Map();
  const messages = [];
  const { RetranscribeDialog } = loadTsModule(fileURLToPath(new URL('../../src/components/MeetingDetails/RetranscribeDialog.tsx', import.meta.url)), {
    react: hooks.react,
    '@tauri-apps/api/core': { invoke: async command => {
      calls.push(command);
      if (editStateFails) throw new Error('Synthetic edit state failure');
      if (command === 'cancel_retranscription_command') return cancelAccepted;
      return { has_edits: true };
    } },
    '@tauri-apps/api/event': { listen: async (event, callback) => { listeners.set(event, callback); return () => listeners.delete(event); } },
    sonner: { toast: { success: message => messages.push(message), info() {} } },
    '@/contexts/ConfigContext': { useTranscriptionConfig: () => ({ selectedLanguage: 'auto', transcriptModelConfig: null }) },
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
  await view.listeners.get('retranscription-complete')({ payload: {
    meeting_id: 'synthetic-meeting', segments_count: 3, duration_seconds: 10,
    language: 'de', file_warning: true,
  } });
  const content = text(view.render());
  assert.match(content, /Retranscription complete/);
  assert.match(content, /recording-folder copy needs updating/);
  assert.doesNotMatch(content, /Retranscription failed/);
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
    meeting_id: 'synthetic-meeting', segments_count: 3, duration_seconds: 10, language: 'de',
  } });
  assert.match(text(view.render()), /Retranscription complete/);
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
    assert.match(text(view.render()), new RegExp(`${name} complete`));
    view.hooks.unmount();
  });

  test(`${hasTranscript ? 'replacement' : 'first'} transcription uses the matching failure title`, async () => {
    const view = await setup(hasTranscript);
    await view.listeners.get('retranscription-error')({ payload: { meeting_id: 'synthetic-meeting', error: 'Synthetic failure' } });
    assert.match(text(view.render()), new RegExp(`${hasTranscript ? 'Retranscription' : 'Transcription'} failed`));
    view.hooks.unmount();
  });
}
