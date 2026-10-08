import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness } from './hook-harness.mjs';

const component = name => fileURLToPath(new URL(`../../src/components/${name}.tsx`, import.meta.url));
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
const find = (tree, type) => nodes(tree).find(node => node.type === type);

function panel(overrides = {}) {
  const hooks = createHookHarness();
  const { TranscriptPanel } = loadTsModule(component('MeetingDetails/TranscriptPanel'), {
    react: hooks.react,
    '@/components/VirtualizedTranscriptView': stubs(['VirtualizedTranscriptView']),
    './TranscriptButtonGroup': stubs(['TranscriptButtonGroup']),
    './MeetingBookmarks': stubs(['MeetingBookmarks']),
    './TranscriptCorrections': stubs(['TranscriptCorrections']),
    '@/components/ui/button': { Button: 'button' },
    '@/components/ui/textarea': { Textarea: 'textarea' },
  });
  const props = {
    transcripts: [], customPrompt: '', isRecording: false,
    meetingId: 'synthetic-meeting', meetingFolderPath: 'synthetic-recording', audioStatus: 'found',
    onPromptChange() {}, onCopyTranscript() {}, onOpenMeetingFolder: async () => {},
    ...overrides,
  };
  return { hooks, props, render: () => hooks.render(() => TranscriptPanel(props)) };
}

test('saved audio offers transcription only after a click, sharing the toolbar dialog state', () => {
  const view = panel();
  let tree = view.render();
  const empty = find(tree, 'VirtualizedTranscriptView').props.emptyState;
  assert.match(text(empty), /No transcript yet/);
  assert.match(text(empty), /This meeting's audio is saved but hasn't been transcribed\./);
  assert.equal(text(find(empty, 'button')), 'Transcribe now');
  assert.equal(find(tree, 'TranscriptButtonGroup').props.transcribeOpen, false);
  find(empty, 'button').props.onClick();
  tree = view.render();
  assert.equal(find(tree, 'TranscriptButtonGroup').props.transcribeOpen, true);
  find(tree, 'TranscriptButtonGroup').props.onTranscribeOpenChange(false);
  assert.equal(find(view.render(), 'TranscriptButtonGroup').props.transcribeOpen, false);
  view.hooks.unmount();
});

test('missing audio offers opening the recording folder', async () => {
  let opened = 0;
  const view = panel({ audioStatus: 'missing', onOpenMeetingFolder: async () => { opened++; } });
  const empty = find(view.render(), 'VirtualizedTranscriptView').props.emptyState;
  assert.match(text(empty), /No transcript yet, and no audio file was found in this meeting's folder\./);
  assert.doesNotMatch(text(empty), /Transcribe now/);
  assert.equal(text(find(empty, 'button')), 'Open recording folder');
  await find(empty, 'button').props.onClick();
  assert.equal(opened, 1);
  view.hooks.unmount();
});

test('audio lookup shows loading without an offer or a missing-audio flash', () => {
  const view = panel({ audioStatus: 'loading' });
  const empty = find(view.render(), 'VirtualizedTranscriptView').props.emptyState;
  assert.equal(text(empty), 'Loading meeting…');
  assert.equal(find(empty, 'button'), undefined);
  view.hooks.unmount();
});

test('meeting without a folder explains that no recording is attached', () => {
  const view = panel({ audioStatus: 'missing', meetingFolderPath: null });
  const empty = find(view.render(), 'VirtualizedTranscriptView').props.emptyState;
  assert.equal(text(empty), 'No transcript or recording is attached to this meeting.');
  assert.equal(find(empty, 'button'), undefined);
  view.hooks.unmount();
});

for (const props of [
  { transcripts: [{ id: 'segment-one', text: 'Synthetic speech.' }] },
  { usePagination: true, segments: [], totalCount: 12 },
]) {
  test(`nonempty ${props.usePagination ? 'paginated' : 'loaded'} transcript has no empty state`, () => {
    const view = panel(props);
    assert.equal(find(view.render(), 'VirtualizedTranscriptView').props.emptyState, undefined);
    view.hooks.unmount();
  });
}

function transcriptView(props = {}) {
  const hooks = createHookHarness();
  const { VirtualizedTranscriptView } = loadTsModule(component('VirtualizedTranscriptView'), {
    './ui/button': { Button: 'button' },
    './ui/input': { Input: 'input' },
    react: { ...hooks.react, memo: fn => fn, useReducer: () => [0, () => {}] },
    '@tanstack/react-virtual': { useVirtualizer: () => ({}) },
    '@/hooks/useAutoScroll': { useAutoScroll: () => ({ autoScroll: false, scrollToBottom() {} }) },
    '@/hooks/useTranscriptStreaming': { useTranscriptStreaming: () => ({ streamingSegmentId: null, getDisplayText: segment => segment.text }) },
    'framer-motion': { motion: { div: 'div' }, AnimatePresence: 'Fragment' },
    './ConfidenceIndicator': stubs(['ConfidenceIndicator']),
    './RecordingStatusBar': stubs(['RecordingStatusBar']),
    './ui/tooltip': stubs(['Tooltip', 'TooltipTrigger', 'TooltipContent']),
    './ui/dropdown-menu': {},
  });
  const tree = hooks.render(() => VirtualizedTranscriptView({ segments: [], ...props }));
  hooks.unmount();
  return tree;
}

test('home empty transcript retains the Welcome text without an override', () => {
  const tree = transcriptView();
  assert.match(text(tree), /Welcome to ClawScribe!/);
  assert.match(text(tree), /Start recording to see live transcription/);
});

test('saved meeting override replaces Welcome while live recording retains its status', () => {
  assert.match(text(transcriptView({ emptyState: 'Saved meeting offer' })), /Saved meeting offer/);
  assert.doesNotMatch(text(transcriptView({ emptyState: 'Saved meeting offer' })), /Welcome/);
  const live = transcriptView({ emptyState: 'Saved meeting offer', isRecording: true });
  assert.match(text(live), /Listening for speech/);
  assert.doesNotMatch(text(live), /Saved meeting offer/);
});

test('toolbar opens and closes the controlled transcription dialog', () => {
  const hooks = createHookHarness();
  const { TranscriptButtonGroup } = loadTsModule(component('MeetingDetails/TranscriptButtonGroup'), {
    react: hooks.react,
    '@tauri-apps/api/core': { invoke: async () => null },
    '@tauri-apps/api/event': { listen: async () => () => {} },
    '@/lib/analytics': { trackButtonClick() {} },
    '@/components/ui/button': { Button: 'button' },
    '@/components/ui/textarea': { Textarea: 'textarea' },
    '@/components/ui/button-group': stubs(['ButtonGroup']),
    '@/components/ui/dropdown-menu': {},
    './RetranscribeDialog': stubs(['RetranscribeDialog']),
    './SpeakerDiarizationDialog': stubs(['SpeakerDiarizationDialog']),
  });
  const props = {
    transcriptCount: 0, meetingId: 'synthetic-meeting', meetingFolderPath: 'synthetic-recording',
    transcribeOpen: false, onTranscribeOpenChange: open => { props.transcribeOpen = open; },
    onCopyTranscript() {}, onOpenMeetingFolder: async () => {}, showSpeakerAttribution: false,
  };
  const render = () => hooks.render(() => TranscriptButtonGroup(props));
  nodes(render()).find(node => node.props.title === 'Transcribe saved audio').props.onClick();
  let dialog = find(render(), 'RetranscribeDialog');
  assert.equal(dialog.props.open, true);
  assert.equal(dialog.props.hasTranscript, false);
  dialog.props.onOpenChange(false);
  assert.equal(find(render(), 'RetranscribeDialog').props.open, false);
  props.transcriptCount = 2;
  assert.equal(find(render(), 'RetranscribeDialog').props.hasTranscript, true);
  hooks.unmount();
});
