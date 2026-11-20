import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, flush } from './hook-harness.mjs';

const source = path => fileURLToPath(new URL(`../../src/${path}.tsx`, import.meta.url));
function nodes(node) {
  if (Array.isArray(node)) return node.flatMap(nodes);
  if (!node || typeof node !== 'object') return [];
  if (typeof node.type === 'function') return nodes(node.type(node.props));
  return [node, ...nodes(node.props?.children)];
}
function text(node) {
  if (Array.isArray(node)) return node.map(text).join('');
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  return node ? text(node.props?.children) : '';
}
const longSegments = Array.from({ length: 250 }, (_, index) => ({
  id: `row-${index}`, timestamp: index * 2, endTime: index * 2 + 1, text: 'Synthetic speech.', speaker: 'Me',
}));
function timeline(props = {}) {
  const hooks = createHookHarness();
  const { SpeakerLaneTimeline } = loadTsModule(source('components/MeetingDetails/SpeakerLaneTimeline'), {
    react: { ...hooks.react, memo: fn => fn },
    '@/components/ui/button': { Button: 'button' },
    './SpeakerLabelPopover': { RenameSpeakerPopover: 'RenameSpeakerPopover' },
    '@/components/ui/tooltip': { Tooltip: 'tooltip', TooltipTrigger: 'tooltip-trigger', TooltipContent: 'tooltip-content' },
    'lucide-react': { Activity: 'Activity', Pause: 'Pause', Play: 'Play' },
  });
  const sought = [], played = [];
  const tree = hooks.render(() => SpeakerLaneTimeline({
    segments: [], isAudioReady: true, durationSeconds: 600, currentTime: 150,
    onSeek: time => sought.push(time), onPlayPause: () => played.push(true), ...props,
  }));
  hooks.unmount();
  return { tree, nodes: nodes(tree), sought, played };
}

for (const props of [
  { segments: [] },
  { segments: longSegments, showSpeakerAttribution: false, totalCount: 250 },
  { segments: [{ id: 'untimed', text: 'Synthetic speech.', timestamp: 0 }], totalCount: 1 },
]) {
  test(`playback renders a plain seek bar for ${props.segments.length === 0 ? 'audio only' : props.showSpeakerAttribution === false ? 'disabled attribution' : 'untimed rows'}`, () => {
    const view = timeline(props);
    const play = view.nodes.find(node => node.props['aria-label'] === 'Play recording');
    assert.ok(play);
    play.props.onClick();
    assert.equal(view.played.length, 1);
    const seek = view.nodes.find(node => node.props['aria-label'] === 'Seek recording');
    assert.ok(seek);
    assert.equal(seek.props.disabled, false);
    assert.equal(seek.props.role, 'slider');
    assert.equal(view.nodes.filter(node => node.props['aria-label']?.endsWith(' timeline')).length, 0);
    assert.equal(view.nodes.filter(node => node.props.title?.startsWith('Me ')).length, 0);
    seek.props.onClick({ detail: 1, clientX: 110, currentTarget: { getBoundingClientRect: () => ({ left: 10, width: 400 }) } });
    assert.deepEqual(view.sought, [150]);
    seek.props.onKeyDown({ key: 'End', preventDefault() {} });
    assert.deepEqual(view.sought, [150, 600]);
    assert.ok(view.nodes.some(node => node.props.style?.left === '25%'));
    assert.match(text(view.tree), /10:00/);
  });
}

test('speaker lanes cover all 250 timed rows alongside the playback strip', () => {
  const view = timeline({ segments: longSegments, totalCount: 250, showSpeakerAttribution: true });
  const lane = view.nodes.find(node => node.props['aria-label'] === 'Seek Me timeline');
  assert.ok(lane);
  assert.equal(lane.props.role, 'slider');
  assert.match(text(view.tree), /Me/);
  assert.ok(view.nodes.some(node => node.props['aria-label'] === 'Seek recording'));
  assert.equal(view.nodes.filter(node => node.props.title?.startsWith('Me ')).length, 250);
  assert.equal(view.nodes.filter(node => node.props['data-timeline-segment']).length, 250);
  const lastBar = view.nodes.find(node => node.props.title === 'Me 08:18-08:19');
  assert.equal(lastBar.props.style.left, '83%');
  assert.doesNotMatch(text(view.tree), /250 segments/);
  assert.doesNotMatch(text(view.tree), /100\/250/);
  lane.props.onClick({ detail: 1, clientX: 410, currentTarget: { getBoundingClientRect: () => ({ left: 10, width: 400 }) } });
  assert.deepEqual(view.sought, [600]);
});

test('speaker lanes visibly name and separately group each speaker', () => {
  const segments = [
    { id: 'me', timestamp: 0, endTime: 3, text: 'Synthetic speech.', speaker: ' Me ' },
    { id: 'participants', timestamp: 4, endTime: 8, text: 'Synthetic speech.', speaker: 'Participants' },
    { id: 'unlabeled', timestamp: 9, endTime: 10, text: 'Synthetic speech.', speaker: ' ' },
  ];
  const view = timeline({ segments, showSpeakerAttribution: true });
  for (const speaker of ['Me', 'Participants', 'Unlabeled']) {
    const lane = view.nodes.find(node => node.props['aria-label'] === `Seek ${speaker} timeline`);
    assert.ok(lane, `${speaker} lane is available`);
    assert.match(text(view.tree), new RegExp(speaker));
    assert.equal(nodes(lane).filter(node => node.props.title?.startsWith(`${speaker} `)).length, 1);
  }
});

for (const label of ['Seek recording', 'Seek Me timeline']) {
  test(`${label} ignores keyboard clicks and supports slider navigation`, () => {
    const view = timeline({ segments: longSegments });
    const seek = view.nodes.find(node => node.props['aria-label'] === label);
    assert.ok(seek);
    seek.props.onClick({ detail: 0, clientX: 0, currentTarget: { getBoundingClientRect: () => ({ left: 10, width: 400 }) } });
    assert.deepEqual(view.sought, []);
    let prevented = 0;
    for (const key of ['ArrowLeft', 'ArrowRight', 'ArrowDown', 'ArrowUp', 'Home', 'End']) {
      seek.props.onKeyDown({ key, preventDefault() { prevented++; } });
    }
    assert.deepEqual(view.sought, [145, 155, 145, 155, 0, 600]);
    assert.equal(prevented, 6);
    for (const key of ['Enter', ' ', 'Tab']) seek.props.onKeyDown({ key, preventDefault() { prevented++; } });
    assert.equal(prevented, 6);
    assert.equal(view.sought.length, 6);
    seek.props.onClick({ detail: 1, clientX: 0, currentTarget: { getBoundingClientRect: () => ({ left: 10, width: 0 }) } });
    assert.equal(view.sought.length, 6);
  });
}

test('keyboard seeks clamp to recording bounds', () => {
  for (const [currentTime, key, expected] of [[1, 'ArrowLeft', 0], [599, 'ArrowRight', 600]]) {
    const view = timeline({ currentTime });
    view.nodes.find(node => node.props['aria-label'] === 'Seek recording').props.onKeyDown({ key, preventDefault() {} });
    assert.deepEqual(view.sought, [expected]);
  }
});

for (const showSpeakerAttribution of [true, false]) {
  test(`invalid timing does not remove valid bars with attribution=${showSpeakerAttribution}`, () => {
    const view = timeline({ durationSeconds: 0, showSpeakerAttribution, segments: [
      { id: 'valid', timestamp: 10, endTime: 20, text: 'Synthetic speech.', speaker: 'Me' },
      { id: 'untimed', timestamp: 0, text: 'Synthetic speech.', speaker: 'Me' },
      { id: 'negative', timestamp: -1, endTime: 2, text: 'Synthetic speech.', speaker: 'Me' },
      { id: 'empty', timestamp: 4, endTime: 4, text: 'Synthetic speech.', speaker: 'Me' },
      { id: 'reversed', timestamp: 5, endTime: 2, text: 'Synthetic speech.', speaker: 'Me' },
      { id: 'invalid-start', timestamp: NaN, endTime: 5, text: 'Synthetic speech.', speaker: 'Me' },
      { id: 'invalid-end', timestamp: 0, endTime: Infinity, text: 'Synthetic speech.', speaker: 'Me' },
    ] });
    const bars = view.nodes.filter(node => node.props['data-timeline-segment']);
    assert.equal(bars.length, 1);
    assert.equal(bars[0].props['data-timeline-segment'], 'valid');
    assert.equal(bars[0].props.style.left, '50%');
    const seek = view.nodes.find(node => node.props['aria-label'] === 'Seek recording');
    assert.equal(seek.props.disabled, false);
    assert.equal(seek.props['aria-valuemax'], 20);
    assert.equal(view.nodes.some(node => node.props['aria-label'] === 'Seek Me timeline'), showSpeakerAttribution);
  });
}

test('playing audio shows Pause; unavailable audio renders no player', () => {
  assert.ok(timeline({ isPlaying: true }).nodes.some(node => node.props['aria-label'] === 'Pause recording playback'));
  assert.equal(timeline({ isAudioReady: false }).tree, null);
});

test('an open lane rename survives optimistic merge until explicitly closed', () => {
  const hooks = createHookHarness();
  const { SpeakerLaneTimeline } = loadTsModule(source('components/MeetingDetails/SpeakerLaneTimeline'), {
    react: { ...hooks.react, memo: fn => fn },
    '@/components/ui/button': { Button: 'button' },
    './SpeakerLabelPopover': { RenameSpeakerPopover: 'RenameSpeakerPopover' },
    '@/components/ui/tooltip': { Tooltip: 'tooltip', TooltipTrigger: 'tooltip-trigger', TooltipContent: 'tooltip-content' },
    'lucide-react': { Pause: 'Pause', Play: 'Play' },
  });
  const props = { segments: [
    { id: 'first', timestamp: 0, endTime: 1, speaker: 'A', text: 'Synthetic speech.' },
    { id: 'second', timestamp: 2, endTime: 3, speaker: 'B', text: 'Synthetic speech.' },
  ], isAudioReady: true, onApplySpeakerToMatching() {} };
  const render = () => hooks.render(() => SpeakerLaneTimeline(props));
  const rename = nodes(render()).find(node => node.type === 'RenameSpeakerPopover' && node.props.speaker === 'B');
  rename.props.onOpenChange(true);
  props.segments = props.segments.map(row => ({ ...row, speaker: 'A' }));
  assert.ok(nodes(render()).some(node => node.type === 'RenameSpeakerPopover' && node.props.speaker === 'B'));
  rename.props.onOpenChange(false);
  assert.equal(nodes(render()).some(node => node.type === 'RenameSpeakerPopover' && node.props.speaker === 'B'), false);
  hooks.unmount();
});

async function page(attribution, segments, updatedSegments) {
  // Keep fixture logging out of node:test's subprocess output stream.
  const previousLog = console.log;
  console.log = () => {};
  const hooks = createHookHarness();
  let summaryOptions, copyOptions;
  const previousWindow = globalThis.window;
  globalThis.window = { addEventListener() {}, removeEventListener() {} };
  const audio = { currentTime: 0, duration: 600, error: null, isPlaying: false, play() {}, pause() {}, seek() {} };
  const { default: PageContent } = loadTsModule(source('app/meeting-details/page-content'), {
    react: hooks.react,
    'framer-motion': { motion: { div: 'div' } },
    '@/lib/meetingContext': { getMeetingContext: async () => '' },
    '@/lib/analytics': { trackPageView() {} },
    '@tauri-apps/api/core': { invoke: async command => { assert.equal(command, 'resolve_meeting_audio_file'); return 'synthetic/audio.wav'; } },
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({}) },
    '@/contexts/ConfigContext': { useConfig: () => ({ modelConfig: {} }) },
    '@/hooks/useSourceAttribution': { useSourceAttribution: () => attribution },
    '@/hooks/useAudioPlayer': { useAudioPlayer: () => audio },
    '@/hooks/meeting-details/useMeetingData': { useMeetingData: () => ({ transcripts: [], blockNoteSummaryRef: { current: null } }) },
    '@/hooks/meeting-details/useSummaryGeneration': { useSummaryGeneration: options => { summaryOptions = options; return {}; } },
    '@/hooks/meeting-details/useTemplates': { useTemplates: () => ({}) },
    '@/hooks/meeting-details/useCopyOperations': { useCopyOperations: options => { copyOptions = options; return {}; } },
    '@/hooks/meeting-details/useMeetingOperations': { useMeetingOperations: () => ({}) },
    '@/components/MeetingDetails/TranscriptPanel': { TranscriptPanel: 'TranscriptPanel' },
    '@/components/MeetingDetails/SummaryPanel': { SummaryPanel: 'SummaryPanel' },
    '@/components/MeetingDetails/MeetingHeader': { MeetingHeader: 'MeetingHeader' },
    '@/components/MeetingAttendance': { SavedMeetingAttendance: 'SavedMeetingAttendance' },
    '@/components/Knowledge/EvidenceRoute': { EvidenceRoute: 'EvidenceRoute' },
    '@/components/MeetingDetails/SpeakerLaneTimeline': { SpeakerLaneTimeline: 'SpeakerLaneTimeline' },
  });
  const props = { meeting: { id: 'synthetic-meeting', folder_path: 'synthetic-folder' }, summaryData: null, segments: segments.slice(0, 100), timelineSegments: segments, totalCount: segments.length };
  const render = () => hooks.render(() => PageContent(props));
  try {
    assert.equal(nodes(render()).some(node => node.type === 'SpeakerLaneTimeline'), false);
    await flush();
    let tree = render();
    const beforeAttribution = nodes(tree).find(node => node.type === 'TranscriptPanel').props.showSpeakerAttribution;
    if (updatedSegments) {
      props.timelineSegments = updatedSegments;
      props.segments = updatedSegments.slice(0, 100);
      tree = render();
    }
    return {
      beforeAttribution, summaryOptions, copyOptions,
      timeline: nodes(tree).find(node => node.type === 'SpeakerLaneTimeline'),
      transcript: nodes(tree).find(node => node.type === 'TranscriptPanel'),
      header: nodes(tree).find(node => node.type === 'MeetingHeader'),
    };
  } finally {
    console.log = previousLog;
    hooks.unmount();
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
  }
}

for (const [attribution, segments, expectedAttribution] of [
  [true, [], true], [false, longSegments, true], [true, longSegments, true],
  [false, longSegments.map(row => ({ ...row, speaker: '' })), false],
  [false, longSegments.map((row, index) => ({ ...row, speaker: index === 249 ? 'Speaker 2' : '' })), true],
]) {
  test(`meeting page exposes ready audio with attribution=${attribution} and ${segments.length} rows`, async () => {
    const result = await page(attribution, segments);
    assert.ok(result.timeline);
    assert.equal(result.timeline.props.segments, segments);
    assert.equal(result.timeline.props.showSpeakerAttribution, expectedAttribution);
    assert.equal(result.transcript.props.showSpeakerAttribution, expectedAttribution);
    assert.equal(result.summaryOptions.includeSpeakerLabels, expectedAttribution);
    assert.equal(result.copyOptions.includeSpeakerLabels, expectedAttribution);
    assert.equal(result.timeline.props.totalCount, segments.length);
    assert.equal(result.timeline.props.isAudioReady, true);
    assert.equal(result.transcript.props.segments.length, Math.min(100, segments.length));
    assert.equal(result.header.props.segmentCount, segments.length);
  });
}

test('refreshed speaker labels appear without changing the Beta toggle', async () => {
  const result = await page(false, longSegments.map(row => ({ ...row, speaker: '' })), longSegments);
  assert.equal(result.beforeAttribution, false);
  assert.equal(result.timeline.props.showSpeakerAttribution, true);
  assert.equal(result.transcript.props.showSpeakerAttribution, true);
  assert.equal(result.summaryOptions.includeSpeakerLabels, true);
  assert.equal(result.copyOptions.includeSpeakerLabels, true);
});

test('removing the last label keeps speaker controls available with Beta disabled', async () => {
  const result = await page(false, longSegments, longSegments.map(row => ({ ...row, speaker: '' })));
  assert.equal(result.transcript.props.showSpeakerAttribution, true);
  assert.equal(result.timeline.props.showSpeakerAttribution, true);
});

test('meeting header metadata displays the total segment count', () => {
  const hooks = createHookHarness();
  const { MeetingHeader } = loadTsModule(source('components/MeetingDetails/MeetingHeader'), {
    react: hooks.react,
    'lucide-react': { MoreHorizontal: 'more' },
    '@/components/EditableTitle': { EditableTitle: 'title' },
    './ActionIcon': { ActionIcon: 'icon-button' },
    './ProjectTags': { ProjectTags: 'tags' },
    './MeetingExportButtons': { MeetingExportButtons: 'exports' },
    '@/components/ui/dropdown-menu': {},
  });
  try {
    const tree = hooks.render(() => MeetingHeader({ meeting: { id: 'synthetic-meeting', created_at: '2026-01-01T12:00:00Z' }, title: 'Synthetic meeting', segmentCount: 250 }));
    assert.match(text(tree), /250 segments/);
    assert.doesNotMatch(text(tree), /100\/250/);
  } finally {
    hooks.unmount();
  }
});
