import assert from 'node:assert/strict';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';

// Test production control handlers with deterministic hooks, not a browser.
function createView(overrides = {}) {
  const slots = [];
  let cursor = 0;
  const errors = [];
  const jsx = (type, props) => ({ type, props: props ?? {} });
  const react = {
    memo: component => component,
    useMemo: compute => compute(),
    useEffect() {},
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
    './ui/tooltip': Object.fromEntries(['Tooltip', 'TooltipTrigger', 'TooltipContent'].map(name => [name, name])),
    './ui/dropdown-menu': Object.fromEntries(['DropdownMenu', 'DropdownMenuContent', 'DropdownMenuItem', 'DropdownMenuLabel', 'DropdownMenuSeparator', 'DropdownMenuSub', 'DropdownMenuSubContent', 'DropdownMenuSubTrigger', 'DropdownMenuTrigger'].map(name => [name, name])),
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

test('failed custom speaker save preserves the input and shows an actionable error', async () => {
  const view = createView({ onSpeakerChange: async () => { throw new Error('simulated failure'); } });
  let nodes = view.render();
  find(nodes, node => node.type === 'input').props.onChange({ target: { value: 'Alex' } });
  nodes = view.render();
  find(nodes, node => node.type === 'button' && node.props.children === 'Set').props.onClick();
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
  find(view.render(), node => node.type === 'button' && node.props.children === 'Set').props.onClick();
  await new Promise(setImmediate);
  assert.deepEqual(calls, [['one', 'Alex']]);
  assert.equal(find(view.render(), node => node.type === 'input').props.value, '');
});

test('an in-flight speaker save cannot be submitted twice before React renders', async () => {
  let resolveSave;
  let saves = 0;
  const view = createView({ onSpeakerChange: () => { saves++; return new Promise(resolve => { resolveSave = resolve; }); } });
  const item = find(view.render(), node => node.type === 'DropdownMenuItem' && node.props.children === 'Participants');
  const first = item.props.onSelect();
  const second = item.props.onSelect();
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
