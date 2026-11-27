import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';

// Test production control handlers with deterministic hooks, not a browser.
function createView(overrides = {}, voiceOption) {
  const slots = [];
  let cursor = 0;
  const errors = [];
  const jsx = (type, props) => ({ type, props: props ?? {} });
  const react = {
    memo: component => component,
    useMemo: compute => compute(),
    useCallback: callback => callback,
    useEffect() {},
    useSyncExternalStore: (_subscribe, getSnapshot) => getSnapshot(),
    startTransition: action => action(),
    useRef(value) { return slots[cursor++] ??= { current: value }; },
    useId() { const index = cursor++; return `speaker-${index}`; },
    useReducer() { return [0, () => {}]; },
    useState(value) {
      const index = cursor++;
      const state = slots[index] ??= { value };
      return [state.value, next => { state.value = typeof next === 'function' ? next(state.value) : next; }];
    },
  };
  const mocks = {
    react,
    'react/jsx-runtime': { jsx, jsxs: jsx, Fragment: 'Fragment' },
    'sonner': { toast: { error: message => errors.push(message) } },
    '@tanstack/react-virtual': { useVirtualizer: () => ({ getTotalSize: () => 60 }) },
    '@/hooks/useAutoScroll': { useAutoScroll: () => ({ autoScroll: false, scrollToBottom: () => {} }) },
    '@/hooks/useTranscriptStreaming': { useTranscriptStreaming: () => ({ streamingSegmentId: null, getDisplayText: segment => segment.text }) },
    'framer-motion': { motion: { div: 'div' }, AnimatePresence: 'Fragment' },
    './ConfidenceIndicator': { ConfidenceIndicator: 'ConfidenceIndicator' },
    './RecordingStatusBar': { RecordingStatusBar: 'RecordingStatusBar' },
    './ui/button': { Button: 'button' },
    '@/components/ui/button': { Button: 'button' },
    '@/components/ui/checkbox': { Checkbox: 'checkbox' },
    '@/components/ui/input': { Input: 'input' },
    '@/components/ui/separator': { Separator: 'separator' },
    '@/components/ui/popover': Object.fromEntries(['Popover', 'PopoverContent', 'PopoverTrigger'].map(name => [name, name])),
    './ui/tooltip': Object.fromEntries(['Tooltip', 'TooltipTrigger', 'TooltipContent'].map(name => [name, name])),
    './SpeakerVoiceOptions': { useSpeakerVoiceOption: speaker => voiceOption?.speaker === speaker ? voiceOption : undefined },
  };
  const modulePath = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../src/components/VirtualizedTranscriptView.tsx');
  const { VirtualizedTranscriptView } = loadTsModule(modulePath, mocks);
  const props = {
    segments: [{ id: 'one', timestamp: 12, text: 'Er kommt um 10 Uhr.', speaker: 'Me' }],
    onSpeakerChange: async () => {},
    ...overrides,
  };
  const flatten = element => {
    if (!element || typeof element !== 'object') return [];
    if (Array.isArray(element)) return element.flatMap(flatten);
    if (typeof element.type === 'function') return flatten(element.type(element.props));
    return [element, ...flatten(element.props.children)];
  };
  return {
    errors,
    render() { cursor = 0; return flatten(VirtualizedTranscriptView(props)); },
  };
}

function find(nodes, predicate) {
  const node = nodes.find(predicate);
  assert.ok(node, 'Expected control was not rendered');
  return node;
}

test('native timestamp button is keyboard accessible without nesting speaker controls in a button', () => {
  const times = [];
  const view = createView({ onSeekToTime: time => times.push(time) });
  const nodes = view.render();
  const row = find(nodes, node => node.props.id === 'segment-one');
  assert.equal(row.props.role, undefined);
  assert.equal(row.props.onClick, undefined);
  const button = find(nodes, node => node.type === 'button' && node.props['aria-label']?.startsWith('Play recording'));
  assert.equal(button.props.disabled, false);
  button.props.onClick();
  assert.deepEqual(times, [12]);
  const input = find(nodes, node => node.type === 'input');
  assert.ok(nodes.some(node => node.type === 'label' && node.props.htmlFor === input.props.id));
});

test('speaker highlighting keeps other rows visible and confirmed attendees can label an unlabeled segment', async () => {
  const assigned = [];
  const view = createView({
    segments: [
      { id: 'one', timestamp: 12, text: 'One.', speaker: ' Ada ' },
      { id: 'two', timestamp: 14, text: 'Two.' },
    ],
    sourceAttributionEnabled: false, highlightedSpeaker: 'Ada', attendeeNames: ['Grace Hopper'],
    onSpeakerChange: async (id, name) => { assigned.push([id, name]); },
  });
  const nodes = view.render();
  assert.equal(find(nodes, node => node.props.id === 'segment-one').props['data-speaker-highlighted'], true);
  assert.equal(find(nodes, node => node.props.id === 'segment-two').props['data-speaker-highlighted'], undefined);
  const second = find(nodes, node => node.props['aria-label']?.includes('00:14, currently unlabeled'));
  assert.equal(second.props.children[1].props.children, 'Add speaker');
  const attendeeOptions = nodes.filter(node => node.props['aria-label'] === 'Assign Grace Hopper');
  await attendeeOptions.at(-1).props.onClick();
  assert.deepEqual(assigned, [['two', 'Grace Hopper']]);
});

test('failed custom speaker save preserves the input and shows an actionable error', async () => {
  const view = createView({ onSpeakerChange: async () => { throw new Error('simulated failure'); } });
  let nodes = view.render();
  find(nodes, node => node.type === 'input').props.onChange({ target: { value: 'Alex' } });
  nodes = view.render();
  find(nodes, node => node.type === 'form').props.onSubmit({ preventDefault() {} });
  await new Promise(setImmediate);
  nodes = view.render();
  assert.equal(find(nodes, node => node.type === 'input').props.value, 'Alex');
  assert.equal(view.errors.length, 1);
  assert.match(view.errors[0], /try again/i);
});

test('successful custom speaker save clears the input only after persistence', async () => {
  const calls = [];
  const view = createView({ onSpeakerChange: async (...args) => calls.push(args) });
  find(view.render(), node => node.type === 'input').props.onChange({ target: { value: 'Alex' } });
  find(view.render(), node => node.type === 'form').props.onSubmit({ preventDefault() {} });
  await new Promise(setImmediate);
  assert.deepEqual(calls, [['one', 'Alex']]);
  assert.equal(find(view.render(), node => node.type === 'input').props.value, '');
});

test('an in-flight speaker save cannot be submitted twice before React renders', async () => {
  let resolveSave;
  let saves = 0;
  const view = createView({ onSpeakerChange: () => { saves++; return new Promise(resolve => { resolveSave = resolve; }); } });
  const item = find(view.render(), node => node.type === 'button' && node.props['aria-label'] === 'Assign Participants');
  const first = item.props.onClick();
  const second = item.props.onClick();
  assert.equal(saves, 1);
  resolveSave();
  await Promise.all([first, second]);
});

test('shows a live-follow recovery button only during an active recording', () => {
  const live = createView({ isRecording: true }).render();
  assert.ok(live.some(node => node.type === 'button' && node.props.children === 'Jump to live transcript'));
  const saved = createView({ isRecording: false }).render();
  assert.ok(!saved.some(node => node.props.children === 'Jump to live transcript'));
});

test('choosing an existing speaker changes only this row and closes after saving', async () => {
  const single = [], matching = [];
  const view = createView({
    speakerSegments: [{ timestamp: 0, speaker: 'Me' }, { timestamp: 20, speaker: 'Alex' }],
    onSpeakerChange: async (...args) => single.push(args),
    onApplySpeakerToMatching: (...args) => matching.push(args),
  });
  find(view.render(), node => node.type === 'Popover').props.onOpenChange(true);
  find(view.render(), node => node.props['aria-label'] === 'Assign Alex').props.onClick();
  await new Promise(setImmediate);
  assert.deepEqual(single, [['one', 'Alex']]);
  assert.deepEqual(matching, []);
  assert.equal(find(view.render(), node => node.type === 'Popover').props.open, false);
});

test('remove label changes only this row and unlabeled rows offer Add speaker', async () => {
  const calls = [];
  const view = createView({ onSpeakerChange: async (...args) => calls.push(args), onApplySpeakerToMatching: () => assert.fail('must not change matching rows') });
  find(view.render(), node => node.props.children === 'Remove label').props.onClick();
  await new Promise(setImmediate);
  assert.deepEqual(calls, [['one', null]]);
  const unlabeled = createView({ segments: [{ id: 'one', timestamp: 17, text: 'Synthetic speech.' }] }).render();
  assert.ok(unlabeled.some(node => node.props.children === 'Add speaker'));
  assert.ok(unlabeled.some(node => node.props['aria-label'] === 'Change speaker for segment at 00:17, currently unlabeled'));
});

test('row rename uses full timeline count, detects merge and saves matching labels only', async () => {
  const calls = [];
  const view = createView({
    speakerSegments: [{ timestamp: 0, speaker: 'Me' }, { timestamp: 2, speaker: 'Me' }, { timestamp: 4, speaker: 'Alex' }],
    onSpeakerChange: () => assert.fail('rename must not save one row'),
    onApplySpeakerToMatching: async (...args) => calls.push(args),
  });
  find(view.render(), node => node.type === 'Popover').props.onOpenChange(true);
  find(view.render(), node => node.type === 'button' && Array.isArray(node.props.children) && node.props.children[0] === 'Rename ').props.onClick();
  let nodes = view.render();
  assert.equal(find(nodes, node => node.type === 'input').props.value, 'Me');
  assert.ok(nodes.some(node => node.props.children === 'Applies to 2 segments'));
  find(nodes, node => node.type === 'input').props.onChange({ target: { value: ' Alex ' } });
  nodes = view.render();
  assert.ok(nodes.some(node => node.props.children === 'Merges with Alex'));
  assert.ok(nodes.some(node => node.type === 'button' && node.props.children === 'Merge'));
  find(nodes, node => node.type === 'form').props.onSubmit({ preventDefault() {} });
  await new Promise(setImmediate);
  assert.deepEqual(calls, [['Me', 'Alex', false]]);
  assert.equal(find(view.render(), node => node.type === 'Popover').props.open, false);
});

test('a voice suggestion is a draft and remembering the voice requires an explicit choice', async () => {
  const calls = [];
  const view = createView({
    segments: [{ id: 'one', timestamp: 12, text: 'Synthetic speech.', speaker: 'Speaker 1' }],
    onApplySpeakerToMatching: async (...args) => calls.push(args),
  }, { speaker: 'Speaker 1', canRemember: true, suggestedName: 'Ada' });
  find(view.render(), node => node.type === 'Popover').props.onOpenChange(true);
  find(view.render(), node => node.type === 'button' && node.props.children?.[0] === 'Rename ').props.onClick();

  const suggestion = find(view.render(), node => node.type === 'button' && node.props.children?.[0] === 'Use suggested name ');
  suggestion.props.onClick();
  assert.deepEqual(calls, []);
  assert.equal(find(view.render(), node => node.type === 'input').props.value, 'Ada');
  const choice = find(view.render(), node => node.type === 'checkbox');
  assert.equal(choice.props.checked, false);
  choice.props.onCheckedChange(true);
  find(view.render(), node => node.type === 'form').props.onSubmit({ preventDefault() {} });
  await new Promise(setImmediate);
  assert.deepEqual(calls, [['Speaker 1', 'Ada', true]]);
});

test('visible speakers remain available when the timeline has not loaded', () => {
  const view = createView({ speakerSegments: [] });
  assert.ok(view.render().some(node => node.props['aria-label'] === 'Assign Me'));
});

test('saving disables every speaker control and a failure leaves the popover open', async () => {
  let rejectSave;
  const view = createView({ onSpeakerChange: () => new Promise((resolve, reject) => { rejectSave = reject; }) });
  find(view.render(), node => node.type === 'Popover').props.onOpenChange(true);
  find(view.render(), node => node.props['aria-label'] === 'Assign Participants').props.onClick();
  let nodes = view.render();
  for (const node of nodes.filter(node => node.type === 'input' || node.type === 'button' && !node.props['aria-label']?.startsWith('Play recording'))) {
    assert.equal(node.props.disabled, true);
  }
  rejectSave(new Error('Synthetic failure'));
  await new Promise(setImmediate);
  nodes = view.render();
  assert.equal(find(nodes, node => node.type === 'Popover').props.open, true);
  assert.equal(view.errors.length, 1);
});

test('speaker option arrows move focus without handling keys from the text input', () => {
  const nodes = createView().render();
  const group = find(nodes, node => node.props['aria-label'] === 'Meeting speakers and confirmed attendees');
  const focused = [];
  const buttons = [0, 1].map(index => ({ focus: () => focused.push(index) }));
  let prevented = 0;
  group.props.onKeyDown({ key: 'ArrowDown', target: buttons[0], currentTarget: { querySelectorAll: () => buttons }, preventDefault() { prevented++; } });
  group.props.onKeyDown({ key: 'ArrowUp', target: buttons[0], currentTarget: { querySelectorAll: () => buttons }, preventDefault() { prevented++; } });
  assert.deepEqual(focused, [1, 1]);
  assert.equal(prevented, 2);
  assert.equal(find(nodes, node => node.type === 'input').props.onKeyDown, undefined);
});
