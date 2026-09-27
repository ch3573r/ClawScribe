import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

test('recording retry cancels detection once and waits for its slot to be released', async () => {
  const calls = [];
  const cancelled = deferred();
  let active = 'synthetic-meeting';
  let action;
  const { offerSpeakerDetectionCancellation } = loadTsModule(fileURLToPath(new URL('../../src/lib/speakerDetectionCancellation.ts', import.meta.url)), {
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push(command);
      if (command === 'active_speaker_diarization_command') return active;
      assert.equal(args.meetingId, active);
      await cancelled.promise;
      active = null;
    } },
    sonner: { toast: {
      error: (_message, options) => { action = options.action; },
      loading: () => 1, dismiss() {},
    } },
  });
  await offerSpeakerDetectionCancellation('Speaker detection is running', async () => { assert.equal(active, null); calls.push('record'); });
  assert.equal(action.label, 'Cancel speaker detection and record');
  const first = action.onClick();
  await action.onClick();
  assert.equal(calls.filter(call => call === 'cancel_speaker_diarization_command').length, 1);
  assert.ok(!calls.includes('record'));
  cancelled.resolve();
  await first;
  assert.equal(calls.at(-1), 'record');
});

test('the detection dialog exposes cancellation and disables duplicate requests', () => {
  const { SpeakerDiarizationDialog } = loadTsModule(fileURLToPath(new URL('../../src/components/MeetingDetails/SpeakerDiarizationDialog.tsx', import.meta.url)), {
    'lucide-react': {},
    '@/components/ui/dialog': Object.fromEntries(['Dialog', 'DialogContent', 'DialogDescription', 'DialogFooter', 'DialogHeader', 'DialogTitle'].map(name => [name, name])),
    '@/components/ui/button': { Button: 'Button' },
  });
  let cancelled = 0;
  const props = { open: true, onOpenChange() {}, isProcessing: true, isCancelling: false, onCancel: () => cancelled++, progress: null, result: null, error: null, speakerMode: null, onClearError() {} };
  const findButton = node => {
    if (!node || typeof node !== 'object') return undefined;
    if (node.type === 'Button' && node.props.onClick === props.onCancel) return node;
    return [node.props?.children].flat(Infinity).map(findButton).find(Boolean);
  };
  const button = findButton(SpeakerDiarizationDialog(props));
  assert.equal(button.props.children, 'Cancel speaker detection');
  button.props.onClick();
  assert.equal(cancelled, 1);
  assert.equal(findButton(SpeakerDiarizationDialog({ ...props, isCancelling: true })).props.disabled, true);
});

test('a background detection can be cancelled from the meeting toolbar after navigation', async () => {
  const hooks = createHookHarness();
  const calls = [];
  const names = list => Object.fromEntries(list.map(name => [name, name]));
  const { TranscriptButtonGroup } = loadTsModule(fileURLToPath(new URL('../../src/components/MeetingDetails/TranscriptButtonGroup.tsx', import.meta.url)), {
    react: hooks.react,
    '@/components/ui/button': { Button: 'Button' },
    '@/components/ui/button-group': { ButtonGroup: 'ButtonGroup' },
    '@/components/ui/dropdown-menu': names(['DropdownMenu', 'DropdownMenuContent', 'DropdownMenuItem', 'DropdownMenuLabel', 'DropdownMenuSeparator', 'DropdownMenuTrigger']),
    'lucide-react': {},
    '@tauri-apps/api/core': { invoke: async (command, args) => { calls.push([command, args]); return command === 'active_speaker_diarization_command' ? 'synthetic-meeting' : undefined; } },
    '@tauri-apps/api/event': { listen: async () => () => {} },
    sonner: { toast: { error() {}, success() {}, info() {} } },
    '@/lib/analytics': { default: { trackButtonClick() {} } },
    './RetranscribeDialog': { RetranscribeDialog: 'RetranscribeDialog' },
    './SpeakerDiarizationDialog': { SpeakerDiarizationDialog: 'SpeakerDiarizationDialog' },
  });
  const render = () => hooks.render(() => TranscriptButtonGroup({ transcriptCount: 2, meetingId: 'synthetic-meeting', meetingFolderPath: 'synthetic', onCopyTranscript() {}, onOpenMeetingFolder: async () => {} }));
  render();
  await flush();
  const nodes = node => !node || typeof node !== 'object' ? [] : [node, ...[node.props?.children].flat(Infinity).flatMap(nodes)];
  const tree = nodes(render());
  assert.equal(tree.find(node => node.type === 'SpeakerDiarizationDialog').props.open, false);
  const button = tree.find(node => node.type === 'Button' && node.props.children === 'Cancel speaker detection');
  assert.ok(button);
  button.props.onClick();
  await flush();
  assert.equal(calls.filter(([command]) => command === 'cancel_speaker_diarization_command').length, 1);
  assert.equal(nodes(render()).find(node => node.type === 'Button' && node.props.children === 'Cancelling…').props.disabled, true);
  hooks.unmount();
});
