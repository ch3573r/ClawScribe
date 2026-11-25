import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness } from './hook-harness.mjs';

const stubs = names => Object.fromEntries(names.map(name => [name, name]));
function nodes(node) {
  if (Array.isArray(node)) return node.flatMap(nodes);
  if (!node || typeof node !== 'object') return [];
  return [node, ...nodes(node.props?.children)];
}

for (const [label, expected] of [['Keep my labels', true], ['Replace', false]]) {
  test(`speaker detection asks before using ${label.toLowerCase()} on a labeled transcript`, async () => {
    const hooks = createHookHarness();
    const starts = [];
    const { TranscriptButtonGroup } = loadTsModule('src/components/MeetingDetails/TranscriptButtonGroup.tsx', {
      react: hooks.react,
      '@/components/ui/button': { Button: 'button' },
      '@/components/ui/dialog': stubs(['Dialog', 'DialogContent', 'DialogHeader', 'DialogFooter', 'DialogTitle', 'DialogDescription']),
      '@/components/ui/tooltip': stubs(['Tooltip', 'TooltipTrigger', 'TooltipContent']),
      '@/components/ui/dropdown-menu': stubs(['DropdownMenu', 'DropdownMenuContent', 'DropdownMenuItem', 'DropdownMenuLabel', 'DropdownMenuSeparator', 'DropdownMenuTrigger']),
      './ActionIcon': { ActionIcon: 'icon-button' },
      './RetranscribeDialog': { RetranscribeDialog: 'RetranscribeDialog' },
      './SpeakerDiarizationDialog': { SpeakerDiarizationDialog: 'SpeakerDiarizationDialog' },
      '@tauri-apps/api/core': { invoke: async (command, args) => {
        if (command === 'start_speaker_diarization_command') {
          starts.push(args);
          return new Promise(() => {});
        }
        return null;
      } },
      '@tauri-apps/api/event': { listen: async () => () => {} },
      '@/lib/analytics': { __esModule: true, default: { trackButtonClick() {} } },
    });
    const props = {
      meetingId: 'test-meeting', meetingFolderPath: 'saved-recording', transcriptCount: 5,
      hasSpeakerLabels: true, transcribeOpen: false, onTranscribeOpenChange() {}, onCopyTranscript() {}, onOpenMeetingFolder: async () => {},
    };
    const render = () => nodes(hooks.render(() => TranscriptButtonGroup(props)));
    try {
      render().find(node => node.type === 'DropdownMenuItem' && text(node) === '3 speakers').props.onSelect();
      assert.equal(starts.length, 0);
      const dialog = render().find(node => node.type === 'Dialog' && node.props.open);
      assert.ok(dialog);
      nodes(dialog).find(node => node.type === 'button' && text(node) === label).props.onClick();
      assert.equal(starts.length, 1);
      assert.equal(starts[0].numSpeakers, 3);
      assert.equal(starts[0].preserveExistingLabels, expected);
    } finally { hooks.unmount(); }
  });
}
function text(node) {
  if (Array.isArray(node)) return node.map(text).join('');
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  return node ? text(node.props?.children) : '';
}

for (const meetingFolderPath of [null, '', 'synthetic-recording']) for (const transcriptCount of [0, 2]) {
  test(`transcript menu leaves folder opening to the header with ${meetingFolderPath ? 'an available' : 'no'} path and ${transcriptCount} rows`, () => {
    const hooks = createHookHarness();
    const { TranscriptButtonGroup } = loadTsModule(fileURLToPath(new URL('../../src/components/MeetingDetails/TranscriptButtonGroup.tsx', import.meta.url)), {
      react: hooks.react,
      '@/components/ui/button': { Button: 'button' },
      '@/components/ui/dialog': stubs(['Dialog', 'DialogContent', 'DialogHeader', 'DialogFooter', 'DialogTitle', 'DialogDescription']),
      '@/components/ui/dropdown-menu': stubs(['DropdownMenu', 'DropdownMenuContent', 'DropdownMenuItem', 'DropdownMenuLabel', 'DropdownMenuSeparator', 'DropdownMenuTrigger']),
      './ActionIcon': { ActionIcon: 'icon-button' },
      'lucide-react': {},
      '@tauri-apps/api/core': { invoke: async () => null },
      '@tauri-apps/api/event': { listen: async () => () => {} },
      sonner: { toast: { error() {}, success() {}, info() {} } },
      '@/lib/analytics': { default: { trackButtonClick() {} } },
      './RetranscribeDialog': { RetranscribeDialog: 'RetranscribeDialog' },
      './SpeakerDiarizationDialog': { SpeakerDiarizationDialog: 'SpeakerDiarizationDialog' },
    });
    const tree = hooks.render(() => TranscriptButtonGroup({
      transcriptCount, meetingId: 'synthetic-meeting', meetingFolderPath,
      transcribeOpen: false, onTranscribeOpenChange() {}, onCopyTranscript() {},
    }));
    try {
      const item = nodes(tree).find(node => node.type === 'DropdownMenuItem' && text(node) === 'Open recording folder');
      assert.equal(item, undefined);
      const detect = nodes(tree).find(node => node.type === 'DropdownMenuItem' && text(node) === 'Auto detect');
      assert.equal(Boolean(detect), Boolean(meetingFolderPath && transcriptCount > 0));
      if (detect) assert.equal(detect.props.disabled, false);
    } finally {
      hooks.unmount();
    }
  });
}
