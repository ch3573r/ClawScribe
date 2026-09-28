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
    assert.equal(view.nodes.filter(node => node.props['aria-label']?.endsWith(' timeline')).length, 0);
    assert.equal(view.nodes.filter(node => node.props.title?.startsWith('Me ')).length, 0);
    seek.props.onClick({ clientX: 110, currentTarget: { getBoundingClientRect: () => ({ left: 10, width: 400 }) } });
    assert.deepEqual(view.sought, [150]);
    assert.ok(view.nodes.some(node => node.props.style?.left === '25%'));
    assert.match(text(view.tree), /10:00/);
  });
}

test('speaker lanes cover all 250 timed rows and display the total count', () => {
  const view = timeline({ segments: longSegments, totalCount: 250, showSpeakerAttribution: true });
  const lane = view.nodes.find(node => node.props['aria-label'] === 'Seek Me timeline');
  assert.ok(lane);
  assert.equal(view.nodes.filter(node => node.props.title?.startsWith('Me ')).length, 250);
  const lastBar = view.nodes.find(node => node.props.title === 'Me 08:18-08:19');
  assert.equal(lastBar.props.style.left, '83%');
  assert.match(text(view.tree), /250 segments/);
  assert.doesNotMatch(text(view.tree), /100\/250/);
  lane.props.onClick({ clientX: 410, currentTarget: { getBoundingClientRect: () => ({ left: 10, width: 400 }) } });
  assert.deepEqual(view.sought, [600]);
});

test('playing audio shows Pause; unavailable audio renders no player', () => {
  assert.ok(timeline({ isPlaying: true }).nodes.some(node => node.props['aria-label'] === 'Pause recording playback'));
  assert.equal(timeline({ isAudioReady: false }).tree, null);
});

async function page(attribution, segments) {
  const hooks = createHookHarness();
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
    '@/hooks/meeting-details/useSummaryGeneration': { useSummaryGeneration: () => ({}) },
    '@/hooks/meeting-details/useTemplates': { useTemplates: () => ({}) },
    '@/hooks/meeting-details/useCopyOperations': { useCopyOperations: () => ({}) },
    '@/hooks/meeting-details/useMeetingOperations': { useMeetingOperations: () => ({}) },
    '@/components/MeetingDetails/TranscriptPanel': { TranscriptPanel: 'TranscriptPanel' },
    '@/components/MeetingDetails/SummaryPanel': { SummaryPanel: 'SummaryPanel' },
    '@/components/MeetingDetails/MeetingChat': { MeetingChat: 'MeetingChat' },
    '@/components/MeetingDetails/SpeakerLaneTimeline': { SpeakerLaneTimeline: 'SpeakerLaneTimeline' },
  });
  const props = { meeting: { id: 'synthetic-meeting', folder_path: 'synthetic-folder' }, summaryData: null, segments: segments.slice(0, 100), timelineSegments: segments, totalCount: segments.length };
  const render = () => hooks.render(() => PageContent(props));
  try {
    assert.equal(nodes(render()).some(node => node.type === 'SpeakerLaneTimeline'), false);
    await flush();
    const tree = render();
    return {
      timeline: nodes(tree).find(node => node.type === 'SpeakerLaneTimeline'),
      transcript: nodes(tree).find(node => node.type === 'TranscriptPanel'),
    };
  } finally {
    hooks.unmount();
    if (previousWindow === undefined) delete globalThis.window;
    else globalThis.window = previousWindow;
  }
}

for (const [attribution, segments] of [[true, []], [false, longSegments], [true, longSegments]]) {
  test(`meeting page exposes ready audio with attribution=${attribution} and ${segments.length} rows`, async () => {
    const result = await page(attribution, segments);
    assert.ok(result.timeline);
    assert.equal(result.timeline.props.segments, segments);
    assert.equal(result.timeline.props.showSpeakerAttribution, attribution);
    assert.equal(result.timeline.props.totalCount, segments.length);
    assert.equal(result.timeline.props.isAudioReady, true);
    assert.equal(result.transcript.props.segments.length, Math.min(100, segments.length));
  });
}
