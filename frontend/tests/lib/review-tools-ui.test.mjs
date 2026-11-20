import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

const jsx = (type, props) => ({ type, props: props ?? {} });
const expand = node => {
  if (Array.isArray(node)) return node.flatMap(expand);
  if (!node || typeof node !== 'object') return [];
  if (typeof node.type === 'function') return expand(node.type(node.props));
  return [node, ...expand(node.props?.children)];
};
const text = node => typeof node === 'string' || typeof node === 'number' ? String(node)
  : Array.isArray(node) ? node.map(text).join('') : node?.props ? text(node.props.children) : '';
const names = value => Object.fromEntries(value.split(' ').map(name => [name, name]));
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

// ---------------------------------------------------------------------------
// Transcript view: find highlighting, scrolling to matches, follow playback
// ---------------------------------------------------------------------------
function transcriptView(initial) {
  const hooks = createHookHarness();
  const scrolls = [];
  const rows = Array.from({ length: 12 }, (_, index) => ({ id: `row-${index}`, timestamp: index * 10, endTime: index * 10 + 9, text: index === 3 ? 'The Budget is tight.' : `Synthetic line ${index}.` }));
  const { VirtualizedTranscriptView } = loadTsModule('src/components/VirtualizedTranscriptView.tsx', {
    react: { ...hooks.react, memo: component => component, startTransition: action => action(), useReducer: () => [0, () => {}] },
    'react/jsx-runtime': { jsx, jsxs: jsx },
    '@tanstack/react-virtual': { useVirtualizer: options => ({
      getTotalSize: () => 720, getVirtualItems: () => Array.from({ length: options.count }, (_, index) => ({ index, start: index * 60 })),
      measureElement() {}, scrollToIndex: (...args) => scrolls.push(args),
    }) },
    '@/hooks/useAutoScroll': { useAutoScroll: () => ({ autoScroll: false, scrollToBottom() {} }) },
    '@/hooks/useTranscriptStreaming': { useTranscriptStreaming: () => ({ streamingSegmentId: null, getDisplayText: segment => segment.text }) },
    'framer-motion': { motion: { div: 'div' }, AnimatePresence: 'Fragment' },
    './ConfidenceIndicator': { ConfidenceIndicator: 'ConfidenceIndicator' },
    './RecordingStatusBar': { RecordingStatusBar: 'RecordingStatusBar' },
    './ui/button': { Button: 'button' },
    './ui/tooltip': names('Tooltip TooltipTrigger TooltipContent'),
    './MeetingDetails/SpeakerLabelPopover': names('SpeakerLabelPopover SpeakerDot'),
    'lucide-react': { Pencil: 'Pencil' },
  });
  let props = { segments: rows, showSpeakerLabels: false, ...initial };
  return {
    rows, scrolls,
    set: next => { props = { ...props, ...next }; },
    render: () => expand(hooks.render(() => VirtualizedTranscriptView(props))),
    unmount: () => hooks.unmount(),
  };
}

test('find marks every match, rings only the current passage, and leaves other text alone', () => {
  const view = transcriptView({ findQuery: 'budget', activeFindId: 'row-3', findRequest: 1 });
  const nodes = view.render();
  const marks = nodes.filter(node => node.type === 'mark');
  assert.equal(marks.length, 1);
  assert.equal(text(marks[0]), 'Budget');
  assert.match(marks[0].props.className, /bg-warning/);
  const current = nodes.find(node => node.props?.id === 'segment-row-3');
  assert.match(current.props.className, /ring-2/);
  assert.equal(nodes.filter(node => /(^| )ring-2( |$)/.test(node.props?.className ?? '')).length, 1, 'only the current passage has the strong ring');
  view.set({ findQuery: undefined, activeFindId: null });
  assert.equal(view.render().filter(node => node.type === 'mark').length, 0);
  view.unmount();
});

test('each find request scrolls to the current match once, even when the page loads it later', async () => {
  const view = transcriptView({ findQuery: 'budget', activeFindId: 'row-3', findRequest: 1 });
  view.render(); await sleep(90);
  assert.deepEqual(JSON.parse(JSON.stringify(view.scrolls)), [[3, { align: 'center' }]]);
  view.render(); await sleep(90);
  assert.equal(view.scrolls.length, 1, 'rendering again does not scroll again');
  view.set({ activeFindId: 'row-7', findRequest: 2 });
  view.render(); await sleep(90);
  assert.deepEqual(JSON.parse(JSON.stringify(view.scrolls.at(-1))), [7, { align: 'center' }]);
  // A match whose page is not loaded yet waits for it instead of giving up.
  view.set({ activeFindId: 'row-99', findRequest: 3 });
  view.render(); await sleep(90);
  assert.equal(view.scrolls.length, 2);
  view.set({ segments: [...view.rows, { id: 'row-99', timestamp: 990, endTime: 999, text: 'Late page.' }] });
  view.render(); await sleep(90);
  assert.deepEqual(JSON.parse(JSON.stringify(view.scrolls.at(-1))), [12, { align: 'center' }]);
  view.unmount();
});

test('follow playback scrolls when the spoken passage changes, never on every clock tick', async () => {
  const view = transcriptView({ followPlayback: true, activeTime: 51 });
  view.render();
  assert.deepEqual(JSON.parse(JSON.stringify(view.scrolls)), [[5, { align: 'auto' }]]);
  view.set({ activeTime: 53 }); view.render();
  view.set({ activeTime: 58 }); view.render();
  assert.equal(view.scrolls.length, 1, 'same passage, no further scrolling');
  view.set({ activeTime: 61 }); view.render();
  assert.deepEqual(JSON.parse(JSON.stringify(view.scrolls.at(-1))), [6, { align: 'auto' }]);
  view.set({ followPlayback: false, activeTime: 81 }); view.render();
  assert.equal(view.scrolls.length, 2, 'following off leaves the reader in control');
  view.unmount();
});

// ---------------------------------------------------------------------------
// Find bar
// ---------------------------------------------------------------------------
function findBar(props = {}) {
  const hooks = createHookHarness();
  const { TranscriptFind } = loadTsModule('src/components/MeetingDetails/TranscriptFind.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    'lucide-react': names('ChevronDown ChevronUp X'),
    '@/components/ui/button': { Button: 'button' }, '@/components/ui/input': { Input: 'input' },
    '@/components/ui/tooltip': names('Tooltip TooltipTrigger TooltipContent'),
  });
  const calls = [];
  const all = { query: 'budget', onQueryChange: value => calls.push(['query', value]), current: 1, total: 4, focusRequest: 1,
    onStep: direction => calls.push(['step', direction]), onClose: () => calls.push(['close']), ...props };
  const nodes = expand(hooks.render(() => TranscriptFind(all)));
  return { nodes, calls, hooks };
}
const key = (name, extra = {}) => ({ key: name, shiftKey: false, nativeEvent: {}, preventDefault() { this.prevented = true; }, stopPropagation() { this.stopped = true; }, ...extra });

test('the find bar steps with Enter and Shift+Enter, closes with Escape, and reports the position', () => {
  const { nodes, calls, hooks } = findBar();
  const input = nodes.find(node => node.type === 'input');
  assert.equal(input.props['aria-label'], 'Find in transcript');
  const enter = key('Enter'); input.props.onKeyDown(enter);
  input.props.onKeyDown(key('Enter', { shiftKey: true }));
  const escape = key('Escape'); input.props.onKeyDown(escape);
  input.props.onKeyDown(key('Enter', { nativeEvent: { isComposing: true } }));
  assert.deepEqual(calls, [['step', 1], ['step', -1], ['close']]);
  assert.equal(enter.prevented, true);
  assert.equal(escape.stopped, true);
  const status = nodes.find(node => node.props?.role === 'status');
  assert.equal(text(status), '2 of 4');
  assert.equal(status.props['aria-live'], 'polite');
  nodes.find(node => node.props?.['aria-label'] === 'Next match').props.onClick();
  nodes.find(node => node.props?.['aria-label'] === 'Previous match').props.onClick();
  nodes.find(node => node.props?.['aria-label'] === 'Close find').props.onClick();
  assert.deepEqual(calls.slice(3), [['step', 1], ['step', -1], ['close']]);
  hooks.unmount();
});

test('the find bar says when nothing matches and disables stepping', () => {
  const none = findBar({ total: 0, current: -1 });
  assert.equal(text(none.nodes.find(node => node.props?.role === 'status')), 'No matches');
  assert.equal(none.nodes.find(node => node.props?.['aria-label'] === 'Next match').props.disabled, true);
  assert.equal(none.nodes.find(node => node.props?.['aria-label'] === 'Previous match').props.disabled, true);
  const blank = findBar({ query: '', total: 0, current: -1 });
  assert.equal(text(blank.nodes.find(node => node.props?.role === 'status')), '');
  const unselected = findBar({ current: -1, total: 1 });
  assert.equal(text(unselected.nodes.find(node => node.props?.role === 'status')), '1 passage');
  [none, blank, unselected].forEach(view => view.hooks.unmount());
});

test('find distinguishes partial results from a search of the full transcript', () => {
  const noMatches = findBar({ total: 0, current: -1, complete: false });
  const matches = findBar({ total: 4, current: 1, complete: false });
  const status = view => text(view.nodes.find(node => node.props?.role === 'status'));
  assert.equal(status(noMatches), 'No matches in loaded passages');
  assert.equal(status(matches), '2 of 4 in loaded passages');
  [noMatches, matches].forEach(view => view.hooks.unmount());
});

// ---------------------------------------------------------------------------
// Playback bar: speed, follow, shortcuts help, bookmarks on the timeline
// ---------------------------------------------------------------------------
test('the playback options expose speed choices, follow state and shortcut help', () => {
  const hooks = createHookHarness();
  const { PlaybackOptions } = loadTsModule('src/components/MeetingDetails/PlaybackOptions.tsx', {
    'react/jsx-runtime': { jsx, jsxs: jsx },
    'lucide-react': names('Keyboard LocateFixed'),
    '@/components/ui/button': { Button: 'button' },
    '@/components/ui/dropdown-menu': names('DropdownMenu DropdownMenuContent DropdownMenuRadioGroup DropdownMenuRadioItem DropdownMenuTrigger'),
    '@/components/ui/popover': names('Popover PopoverContent PopoverTrigger'),
    '@/components/ui/tooltip': names('Tooltip TooltipTrigger TooltipContent'),
    '@/components/ui/settings-layout': { Kbd: 'kbd' },
  });
  const changes = [], follows = [];
  const nodes = expand(PlaybackOptions({ rate: 1.25, onRateChange: value => changes.push(value), followPlayback: true, onFollowChange: value => follows.push(value) }));
  const speed = nodes.find(node => node.props?.['aria-label']?.startsWith('Playback speed'));
  assert.equal(speed.props['aria-label'], 'Playback speed, 1.25×');
  assert.equal(text(speed), '1.25×');
  const group = nodes.find(node => node.type === 'DropdownMenuRadioGroup');
  assert.equal(group.props.value, '1.25');
  assert.deepEqual(nodes.filter(node => node.type === 'DropdownMenuRadioItem').map(text), ['0.5×', '0.75×', '1×', '1.25×', '1.5×', '1.75×', '2×']);
  group.props.onValueChange('1.5');
  assert.deepEqual(changes, [1.5]);
  const follow = nodes.find(node => node.props?.['aria-label'] === 'Follow playback in the transcript');
  assert.equal(follow.props['aria-pressed'], true);
  follow.props.onClick();
  assert.deepEqual(follows, [false]);
  const help = nodes.filter(node => node.type === 'kbd').map(text);
  assert.ok(help.includes('Space') && help.includes('Ctrl + F') && help.includes('B'));
  assert.ok(nodes.some(node => node.props?.['aria-label'] === 'Playback shortcuts'));
  // Without handlers only the shortcut help remains.
  const bare = expand(PlaybackOptions({}));
  assert.equal(bare.some(node => node.props?.['aria-label']?.startsWith('Playback speed')), false);
  assert.equal(bare.some(node => node.props?.['aria-label'] === 'Follow playback in the transcript'), false);
  hooks.unmount();
});

test('bookmarks appear as keyboard-reachable markers on the timeline and seek when chosen', () => {
  const hooks = createHookHarness();
  const { SpeakerLaneTimeline } = loadTsModule('src/components/MeetingDetails/SpeakerLaneTimeline.tsx', {
    react: { ...hooks.react, memo: component => component },
    'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/components/ui/button': { Button: 'button' },
    './SpeakerLabelPopover': { RenameSpeakerPopover: 'RenameSpeakerPopover' },
    './PlaybackOptions': { PlaybackOptions: 'PlaybackOptions' },
    '@/components/ui/tooltip': names('Tooltip TooltipTrigger TooltipContent'),
    'lucide-react': names('Activity Bookmark Pause Play'),
  });
  const sought = [], rates = [];
  const bookmarks = [{ id: 'a', seconds: 150, label: 'Decision' }, { id: 'b', seconds: 9999, label: 'Past the end' }, { id: 'c', seconds: -5, label: 'Negative' }];
  const nodes = expand(hooks.render(() => SpeakerLaneTimeline({
    segments: [], isAudioReady: true, durationSeconds: 600, currentTime: 0, onSeek: time => sought.push(time),
    bookmarks, rate: 1.5, onRateChange: value => rates.push(value), followPlayback: true, onFollowChange() {},
  })));
  const markers = nodes.filter(node => node.props?.['aria-label']?.startsWith('Go to bookmark'));
  assert.equal(markers.length, 1, 'markers outside the recording are not drawn');
  assert.equal(markers[0].props['aria-label'], 'Go to bookmark at 00:02:30: Decision');
  assert.equal(markers[0].props.style.left, '25%');
  assert.equal(markers[0].props.type, 'button');
  markers[0].props.onClick();
  assert.deepEqual(sought, [150]);
  const options = nodes.find(node => node.type === 'PlaybackOptions');
  assert.equal(options.props.rate, 1.5);
  options.props.onRateChange(2);
  assert.deepEqual(rates, [2]);
  hooks.unmount();
});

// ---------------------------------------------------------------------------
// Bookmarks and notes
// ---------------------------------------------------------------------------
function bookmarksUi(props = {}, stored = {}) {
  const hooks = createHookHarness();
  const invoked = [], reported = [], toasts = [];
  const storage = new Map(Object.entries(stored));
  let failWrite = false;
  const { MeetingBookmarks } = loadTsModule('src/components/MeetingDetails/MeetingBookmarks.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      invoked.push([command, args]);
      if (command === 'list_meeting_bookmarks') return [{ id: 'one', seconds: 65, label: 'Budget risk' }];
      if (command === 'get_meeting_summary_bookmarks') return storage.get(args.meetingId) ?? false;
      if (command === 'set_meeting_summary_bookmarks') {
        if (failWrite) throw new Error('blocked');
        storage.set(args.meetingId, args.enabled);
      }
    } },
    '@tauri-apps/api/event': { listen: async () => () => {} },
    sonner: { toast: { error: message => toasts.push(message), success() {} } },
    'lucide-react': names('BookmarkPlus'),
    '@/components/ui/button': { Button: 'button' }, '@/components/ui/input': { Input: 'input' }, '@/components/ui/switch': { Switch: 'switch' },
    '@/components/ui/dialog': names('Dialog DialogContent DialogHeader DialogTitle DialogDescription'),
    '@/components/ui/accordion': names('Accordion AccordionContent AccordionItem AccordionTrigger'),
  });
  const render = () => expand(hooks.render(() => MeetingBookmarks({ meetingId: 'meeting-1', currentTime: 42.34, onSeek() {}, onBookmarksChange: value => reported.push(value), ...props })));
  return { render, invoked, reported, toasts, storage, hooks, blockWrites: () => { failWrite = true; } };
}

test('a new note starts at the playback position, submits with Enter, and falls back to a default label', async () => {
  const ui = bookmarksUi({ addOpen: true });
  try {
    ui.render(); await flush();
    let nodes = ui.render();
    const form = nodes.find(node => node.type === 'form');
    const [note, time] = nodes.filter(node => node.type === 'input');
    assert.equal(note.props.autoFocus, true);
    assert.equal(note.props.placeholder, 'Review this');
    assert.equal(note.props.value, '');
    assert.equal(time.props.value, '42.3', 'prefilled from playback');
    assert.match(text(nodes.find(node => node.type === 'dialog-description' || node.type === 'DialogDescription')), /private by default/);
    const submit = { prevented: false, preventDefault() { this.prevented = true; } };
    await form.props.onSubmit(submit);
    assert.equal(submit.prevented, true);
    await flush();
    const add = ui.invoked.find(([command]) => command === 'add_meeting_bookmark');
    assert.deepEqual(JSON.parse(JSON.stringify(add[1])), { meetingId: 'meeting-1', seconds: 42.3, label: 'Review this' });
    nodes = ui.render();
    note.props.onChange({ target: { value: '  Ask legal about the contract  ' } });
    nodes = ui.render();
    await nodes.find(node => node.type === 'form').props.onSubmit({ preventDefault() {} });
    assert.equal(ui.invoked.filter(([command]) => command === 'add_meeting_bookmark').at(-1)[1].label, 'Ask legal about the contract');
  } finally { ui.hooks.unmount(); }
});

test('bookmarks are reported to the page for the timeline', async () => {
  const ui = bookmarksUi();
  try {
    ui.render(); await flush(); ui.render();
    assert.deepEqual(JSON.parse(JSON.stringify(ui.reported.at(-1))), [{ id: 'one', seconds: 65, label: 'Budget risk' }]);
  } finally { ui.hooks.unmount(); }
});

test('using notes as summary priorities is off by default and changes only with the switch', async () => {
  const ui = bookmarksUi();
  const another = bookmarksUi({ meetingId: 'meeting-2' }, { 'meeting-1': true });
  try {
    ui.render(); await flush();
    let toggle = ui.render().find(node => node.type === 'switch');
    assert.equal(toggle.props.checked, false);
    assert.match(toggle.props['aria-label'], /summary priorities/);
    assert.equal(ui.storage.size, 0);
    await toggle.props.onCheckedChange(true);
    assert.equal(ui.storage.get('meeting-1'), true);
    toggle = ui.render().find(node => node.type === 'switch');
    assert.equal(toggle.props.checked, true);
    assert.match(text(ui.render().find(node => node.type === 'accordion-content' || node.type === 'AccordionContent')), /Sends these notes to the summary model.*Off by default/);
    await toggle.props.onCheckedChange(false);
    assert.equal(ui.storage.get('meeting-1'), false);
    another.render(); await flush();
    assert.equal(another.render().find(node => node.type === 'switch').props.checked, false);
  } finally { ui.hooks.unmount(); another.hooks.unmount(); }
});

test('a choice that cannot be stored is reported and leaves the switch off', async () => {
  const ui = bookmarksUi();
  ui.blockWrites();
  try {
    ui.render(); await flush();
    const toggle = ui.render().find(node => node.type === 'switch');
    await toggle.props.onCheckedChange(true);
    assert.equal(ui.render().find(node => node.type === 'switch').props.checked, false);
    assert.deepEqual(ui.toasts, ['Could not save this choice']);
  } finally { ui.hooks.unmount(); }
});

// ---------------------------------------------------------------------------
// Audio player: speed and relative seek
// ---------------------------------------------------------------------------
test('playback speed is applied to the audio, remembered, and restored for the next recording', async () => {
  const previousWindow = globalThis.window;
  const elements = [];
  const storage = new Map();
  class Audio {
    constructor() { this.handlers = new Map(); this.duration = NaN; this.currentTime = 0; this.playbackRate = 1; elements.push(this); }
    addEventListener(name, handler) { this.handlers.set(name, handler); }
    removeEventListener(name) { this.handlers.delete(name); }
    emit(name) { this.handlers.get(name)?.(); }
    load() {} play() { return Promise.resolve(); } pause() {} removeAttribute() {}
  }
  globalThis.window = { Audio, localStorage: { getItem: name => storage.get(name) ?? null, setItem: (name, value) => storage.set(name, value) } };
  const hooks = createHookHarness();
  try {
    const { useAudioPlayer } = loadTsModule('src/hooks/useAudioPlayer.ts', { react: hooks.react, '@tauri-apps/api/core': { convertFileSrc: path => `asset:${path}` } });
    let path = 'one.wav';
    const render = () => hooks.render(() => useAudioPlayer(path));
    render();
    assert.equal(render().rate, 1);
    assert.equal(elements[0].playbackRate, 1);
    render().setRate(1.75);
    assert.equal(render().rate, 1.75);
    assert.equal(elements[0].playbackRate, 1.75);
    assert.equal(storage.get('clawscribe.playbackRate'), '1.75');
    render().setRate(1.3);
    assert.equal(elements[0].playbackRate, 1.25, 'only supported speeds are applied');
    render().setRate(1.75);
    path = 'two.wav'; render();
    assert.equal(elements[1].playbackRate, 1.75, 'a new recording keeps the chosen speed');
    // Relative seeks use the element clock, clamp at both ends and wait for metadata.
    const audio = elements[1];
    await render().seekBy(5);
    audio.duration = 100; audio.emit('loadedmetadata');
    assert.equal(audio.currentTime, 5);
    await render().seekBy(5); await render().seekBy(5);
    assert.equal(audio.currentTime, 15, 'repeated key presses accumulate');
    await render().seekBy(-1000);
    assert.equal(audio.currentTime, 0);
    await render().seekBy(1000);
    assert.equal(audio.currentTime, 100);
    hooks.unmount();
    // A fresh player starts at the remembered speed.
    const second = createHookHarness();
    const { useAudioPlayer: again } = loadTsModule('src/hooks/useAudioPlayer.ts', { react: second.react, '@tauri-apps/api/core': { convertFileSrc: path => path } });
    assert.equal(second.render(() => again('three.wav')).rate, 1.75);
    second.unmount();
  } finally { globalThis.window = previousWindow; }
});

test('a damaged remembered speed falls back to normal speed', () => {
  const previousWindow = globalThis.window;
  globalThis.window = { Audio: class { addEventListener() {} removeEventListener() {} load() {} pause() {} removeAttribute() {} }, localStorage: { getItem: () => 'warp-speed', setItem() { throw new Error('blocked'); } } };
  const hooks = createHookHarness();
  try {
    const { useAudioPlayer } = loadTsModule('src/hooks/useAudioPlayer.ts', { react: hooks.react, '@tauri-apps/api/core': { convertFileSrc: path => path } });
    const player = hooks.render(() => useAudioPlayer('one.wav'));
    assert.equal(player.rate, 1);
    player.setRate(1.5); // storage refuses; the speed still applies for this session
    assert.equal(hooks.render(() => useAudioPlayer('one.wav')).rate, 1.5);
    hooks.unmount();
  } finally { globalThis.window = previousWindow; }
});

// ---------------------------------------------------------------------------
// Transcript panel: find flow
// ---------------------------------------------------------------------------
function panel(initial = {}) {
  const hooks = createHookHarness();
  const stub = name => () => ({ type: name, props: {} });
  const { TranscriptPanel } = loadTsModule('src/components/MeetingDetails/TranscriptPanel.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    sonner: { toast: { error() {} } },
    '@/components/VirtualizedTranscriptView': { VirtualizedTranscriptView: 'VirtualizedTranscriptView' },
    './TranscriptButtonGroup': { TranscriptButtonGroup: 'TranscriptButtonGroup' },
    './MeetingBookmarks': { MeetingBookmarks: 'MeetingBookmarks' },
    './TranscriptCorrections': { TranscriptCorrections: 'TranscriptCorrections' },
    './TranscriptFind': { TranscriptFind: 'TranscriptFind' },
    '@/components/ui/button': { Button: 'button' },
  });
  const rows = [
    { id: 'row-0', timestamp: 0, text: 'Welcome everyone.' },
    { id: 'row-1', timestamp: 10, text: 'The budget is tight.' },
    { id: 'row-2', timestamp: 20, text: 'Other topic.' },
    { id: 'row-3', timestamp: 30, text: 'Budget again, budget twice.' },
  ];
  const reveals = [];
  let props = { transcripts: [], isRecording: false, meetingId: 'meeting-1', meetingFolderPath: 'folder', audioStatus: 'found',
    speakerSegments: rows, onRevealSegment: async (id, index) => { reveals.push([id, index]); }, ...initial };
  const render = () => expand(hooks.render(() => TranscriptPanel(props)));
  const find = nodes => nodes.find(node => node.type === 'TranscriptFind');
  const view = nodes => nodes.find(node => node.type === 'VirtualizedTranscriptView').props;
  return { rows, reveals, render, find, view, set: next => { props = { ...props, ...next }; }, hooks };
}

test('playback follows a late passage without fetching transcript pages the timeline already contains', () => {
  const rows = Array.from({ length: 206 }, (_, index) => ({
    id: String(index), text: 'Passage ' + index, timestamp: index, endTime: index + 0.9,
  }));
  const ui = panel({
    usePagination: true, segments: rows.slice(0, 100), totalCount: rows.length, loadedCount: 100,
    hasMore: true, speakerSegments: rows, timelineComplete: false, followPlayback: true, activeTime: 205.2,
  });
  assert.equal(ui.view(ui.render()).segments.length, 100);
  assert.equal(ui.view(ui.render()).hasMore, true);
  ui.set({ timelineComplete: true });
  const props = ui.view(ui.render());
  const view = transcriptView(props);
  view.render();
  assert.deepEqual(JSON.parse(JSON.stringify(view.scrolls)), [[205, { align: 'auto' }]]);
  assert.equal(props.hasMore, false);
  assert.equal(props.loadedCount, 206);
  view.unmount();
  ui.hooks.unmount();
});

test('Ctrl+F opens the find bar; typing goes to the first match; stepping wraps and loads the page first', async () => {
  const ui = panel();
  ui.render();
  assert.equal(ui.find(ui.render()), undefined);
  assert.equal(ui.view(ui.render()).findQuery, undefined);
  ui.set({ findOpenRequest: 1 }); ui.render();
  let bar = ui.find(ui.render());
  assert.ok(bar, 'the bar opens when the page raises a find request');
  bar.props.onQueryChange('BUDGET'); ui.render();
  await sleep(220); ui.render();
  assert.deepEqual(ui.reveals, [['row-1', 1]]);
  bar = ui.find(ui.render());
  assert.equal(bar.props.total, 2); assert.equal(bar.props.current, 0);
  let transcript = ui.view(ui.render());
  assert.equal(transcript.findQuery, 'BUDGET'); assert.equal(transcript.activeFindId, 'row-1');
  const firstRequest = transcript.findRequest;
  bar.props.onStep(1); await flush(); ui.render();
  transcript = ui.view(ui.render());
  assert.deepEqual(ui.reveals.at(-1), ['row-3', 3]);
  assert.equal(transcript.activeFindId, 'row-3'); assert.ok(transcript.findRequest > firstRequest);
  ui.find(ui.render()).props.onStep(1); await flush(); ui.render();
  assert.equal(ui.view(ui.render()).activeFindId, 'row-1', 'wraps to the first match');
  ui.find(ui.render()).props.onStep(-1); await flush(); ui.render();
  assert.equal(ui.view(ui.render()).activeFindId, 'row-3', 'wraps backwards');
  ui.hooks.unmount();
});

test('loading pages or renaming speakers keeps the search on the same passage', async () => {
  const ui = panel();
  ui.render(); ui.set({ findOpenRequest: 1 }); ui.render(); ui.render();
  ui.find(ui.render()).props.onQueryChange('budget'); ui.render();
  await sleep(220); ui.render();
  ui.find(ui.render()).props.onStep(1); await flush(); ui.render();
  assert.equal(ui.view(ui.render()).activeFindId, 'row-3');
  const before = ui.reveals.length;
  // The page hands over a new reveal function and a new segment array after pages load.
  ui.set({ onRevealSegment: async (id, index) => { ui.reveals.push([id, index]); }, speakerSegments: ui.rows.map(row => ({ ...row, speaker: 'Ana' })) });
  ui.render(); await sleep(220); ui.render();
  assert.equal(ui.reveals.length, before, 'no search restarts');
  assert.equal(ui.view(ui.render()).activeFindId, 'row-3');
  assert.equal(ui.find(ui.render()).props.current, 1);
  // If the passage is edited so it no longer matches, nothing is selected.
  ui.set({ speakerSegments: ui.rows.map(row => row.id === 'row-3' ? { ...row, text: 'Nothing here.' } : row) });
  ui.render(); ui.render();
  assert.equal(ui.view(ui.render()).activeFindId, null);
  assert.equal(ui.find(ui.render()).props.total, 1);
  ui.hooks.unmount();
});

test('Escape closes the find bar and clears the marks; a bookmark request opens the dialog', async () => {
  const previousDocument = globalThis.document;
  const focused = [];
  globalThis.document = { querySelector: selector => ({ focus: () => focused.push(selector) }) };
  const ui = panel();
  try {
    ui.render(); ui.set({ findOpenRequest: 1 }); ui.render(); ui.render();
    ui.find(ui.render()).props.onQueryChange('budget'); ui.render();
    await sleep(220); ui.render();
    ui.find(ui.render()).props.onClose(); ui.render();
    assert.equal(ui.find(ui.render()), undefined);
    const transcript = ui.view(ui.render());
    assert.equal(transcript.findQuery, undefined); assert.equal(transcript.activeFindId, null);
    assert.deepEqual(focused, ['[data-transcript-panel]']);
    assert.equal(ui.render().find(node => node.type === 'MeetingBookmarks').props.addOpen, false);
    ui.set({ bookmarkRequest: 1 }); ui.render();
    assert.equal(ui.render().find(node => node.type === 'MeetingBookmarks').props.addOpen, true);
  } finally { globalThis.document = previousDocument; ui.hooks.unmount(); }
});

// ---------------------------------------------------------------------------
// Meeting page: shortcut wiring
// ---------------------------------------------------------------------------
async function meetingPage({ audioReady = true } = {}) {
  const previousLog = console.log, previousWindow = globalThis.window;
  console.log = () => {};
  globalThis.window = { addEventListener() {}, removeEventListener() {} };
  const hooks = createHookHarness();
  const calls = [];
  let shortcuts;
  const audio = { currentTime: 20, duration: audioReady ? 600 : 0, error: null, isPlaying: false, rate: 1.25,
    play: async () => { calls.push('play'); }, pause: () => calls.push('pause'), seek: async () => {},
    seekBy: async seconds => { calls.push(['seekBy', seconds]); }, setRate: rate => { calls.push(['rate', rate]); } };
  const { default: PageContent } = loadTsModule('src/app/meeting-details/page-content.tsx', {
    react: hooks.react,
    'framer-motion': { motion: { div: 'div' } },
    '@/lib/meetingContext': { getMeetingContext: async () => '' },
    '@/lib/analytics': { trackPageView() {} },
    '@tauri-apps/api/core': { invoke: async () => 'synthetic/audio.wav' },
    '@/components/Sidebar/SidebarProvider': { useSidebar: () => ({}) },
    '@/contexts/ConfigContext': { useConfig: () => ({ modelConfig: {} }) },
    '@/hooks/useSourceAttribution': { useSourceAttribution: () => false },
    '@/hooks/useAudioPlayer': { useAudioPlayer: () => audio },
    '@/hooks/usePlaybackShortcuts': { usePlaybackShortcuts: handlers => { shortcuts = handlers; } },
    '@/hooks/meeting-details/useMeetingData': { useMeetingData: () => ({ transcripts: [], blockNoteSummaryRef: { current: null } }) },
    '@/hooks/meeting-details/useSummaryGeneration': { useSummaryGeneration: () => ({}) },
    '@/hooks/meeting-details/useTemplates': { useTemplates: () => ({}) },
    '@/hooks/meeting-details/useCopyOperations': { useCopyOperations: () => ({}) },
    '@/hooks/meeting-details/useMeetingOperations': { useMeetingOperations: () => ({}) },
    '@/components/MeetingDetails/TranscriptPanel': { TranscriptPanel: 'TranscriptPanel' },
    '@/components/MeetingDetails/SummaryPanel': { SummaryPanel: 'SummaryPanel' },
    '@/components/MeetingDetails/MeetingHeader': { MeetingHeader: 'MeetingHeader' },
    '@/components/MeetingAttendance': { SavedMeetingAttendance: 'SavedMeetingAttendance' },
    '@/components/ui/button': { Button: 'button' },
    'next/link': { default: 'a', __esModule: true },
    '@/components/Knowledge/EvidenceRoute': { EvidenceRoute: 'EvidenceRoute' },
    '@/components/MeetingDetails/SpeakerLaneTimeline': { SpeakerLaneTimeline: 'SpeakerLaneTimeline' },
  });
  const props = { meeting: { id: 'synthetic-meeting', folder_path: 'synthetic-folder' }, summaryData: null, segments: [], timelineSegments: [], totalCount: 0 };
  const render = () => expand(hooks.render(() => PageContent(props)));
  render(); await flush(); render();
  return { calls, audio, render, shortcuts: () => shortcuts, restore: () => { console.log = previousLog; globalThis.window = previousWindow; hooks.unmount(); } };
}

test('page shortcuts drive the player, follow, bookmarks and find, and only offer playback keys with ready audio', async () => {
  const ui = await meetingPage();
  try {
    const keys = ui.shortcuts();
    keys.toggle(); keys.seek({ seconds: -5 }); keys.seek({ seconds: 15 });
    keys.rate({ direction: 1 }); keys.rate({ direction: -1 });
    assert.deepEqual(JSON.parse(JSON.stringify(ui.calls)), ['play', ['seekBy', -5], ['seekBy', 15], ['rate', 1.5], ['rate', 1]]);
    // Follow starts on and is switched by the key; the transcript only follows while audio plays.
    assert.equal(ui.render().find(node => node.type === 'SpeakerLaneTimeline').props.followPlayback, true);
    assert.equal(ui.render().find(node => node.type === 'TranscriptPanel').props.followPlayback, false, 'paused');
    ui.audio.isPlaying = true;
    assert.equal(ui.render().find(node => node.type === 'TranscriptPanel').props.followPlayback, true);
    ui.render().find(node => node.type === 'TranscriptPanel').props.onFollowStop();
    assert.equal(ui.render().find(node => node.type === 'SpeakerLaneTimeline').props.followPlayback, false, 'scrolling by hand turns it off');
    ui.shortcuts().follow();
    assert.equal(ui.render().find(node => node.type === 'SpeakerLaneTimeline').props.followPlayback, true);
    const before = ui.render().find(node => node.type === 'TranscriptPanel').props;
    ui.shortcuts().bookmark(); ui.shortcuts().find();
    const after = ui.render().find(node => node.type === 'TranscriptPanel').props;
    assert.equal(after.bookmarkRequest, before.bookmarkRequest + 1);
    assert.equal(after.findOpenRequest, before.findOpenRequest + 1);
    const timeline = ui.render().find(node => node.type === 'SpeakerLaneTimeline').props;
    assert.equal(timeline.rate, 1.25); assert.equal(timeline.onRateChange, ui.audio.setRate);
    ui.render().find(node => node.type === 'TranscriptPanel').props.onBookmarksChange([{ id: 'a', seconds: 9, label: 'Note' }]);
    assert.deepEqual(JSON.parse(JSON.stringify(ui.render().find(node => node.type === 'SpeakerLaneTimeline').props.bookmarks)), [{ id: 'a', seconds: 9, label: 'Note' }]);
  } finally { ui.restore(); }
});

test('without ready audio only find is handled, so Space and the arrows keep their normal meaning', async () => {
  const ui = await meetingPage({ audioReady: false });
  try {
    const keys = ui.shortcuts();
    for (const name of ['toggle', 'seek', 'rate', 'follow', 'bookmark']) assert.equal(keys[name], undefined, name);
    assert.equal(typeof keys.find, 'function');
  } finally { ui.restore(); }
});
