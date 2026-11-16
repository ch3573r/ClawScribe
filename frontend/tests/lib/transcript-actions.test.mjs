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
function text(node) {
  if (Array.isArray(node)) return node.map(text).join('');
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  return node ? text(node.props?.children) : '';
}

for (const meetingFolderPath of [null, '', 'synthetic-recording']) for (const transcriptCount of [0, 2]) {
  test(`transcript menu retains the recording-folder action with ${meetingFolderPath ? 'an available' : 'no'} path and ${transcriptCount} rows`, () => {
    const hooks = createHookHarness();
    const { TranscriptButtonGroup } = loadTsModule(fileURLToPath(new URL('../../src/components/MeetingDetails/TranscriptButtonGroup.tsx', import.meta.url)), {
      react: hooks.react,
      '@/components/ui/button': { Button: 'button' },
      '@/components/ui/dropdown-menu': stubs(['DropdownMenu', 'DropdownMenuContent', 'DropdownMenuItem', 'DropdownMenuLabel', 'DropdownMenuSeparator', 'DropdownMenuTrigger']),
      '@/components/ui/tooltip': stubs(['Tooltip', 'TooltipTrigger', 'TooltipContent']),
      './ActionIcon': { ActionIcon: 'icon-button' },
      'lucide-react': {},
      '@tauri-apps/api/core': { invoke: async () => null },
      '@tauri-apps/api/event': { listen: async () => () => {} },
      sonner: { toast: { error() {}, success() {}, info() {} } },
      '@/lib/analytics': { default: { trackButtonClick() {} } },
      './RetranscribeDialog': { RetranscribeDialog: 'RetranscribeDialog' },
      './SpeakerDiarizationDialog': { SpeakerDiarizationDialog: 'SpeakerDiarizationDialog' },
    });
    let opened = 0;
    const tree = hooks.render(() => TranscriptButtonGroup({
      transcriptCount, meetingId: 'synthetic-meeting', meetingFolderPath,
      transcribeOpen: false, onTranscribeOpenChange() {}, onCopyTranscript() {},
      onOpenMeetingFolder: async () => { opened++; },
    }));
    try {
      const item = nodes(tree).find(node => node.type === 'DropdownMenuItem' && text(node) === 'Open recording folder');
      assert.ok(item);
      const detect = nodes(tree).find(node => node.type === 'DropdownMenuItem' && text(node) === 'Auto detect');
      assert.equal(Boolean(detect), Boolean(meetingFolderPath && transcriptCount > 0));
      if (detect) assert.equal(detect.props.disabled, false);
      assert.equal(item.props.disabled, !meetingFolderPath);
      if (meetingFolderPath) {
        item.props.onSelect();
        assert.equal(opened, 1);
      } else {
        const hint = nodes(tree).find(node => node.type === 'TooltipContent');
        assert.ok(hint);
        assert.match(text(hint), /No recording folder is attached/);
        item.props.onSelect();
        assert.equal(opened, 0);
      }
    } finally {
      hooks.unmount();
    }
  });
}
