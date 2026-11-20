import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const noop = () => {};
const jsx = (type, props) => ({ type, props });
// Expands the small local components so the drawn rows can be inspected.
const expand = node => Array.isArray(node) ? node.flatMap(expand)
  : node && typeof node === 'object'
    ? (typeof node.type === 'function' ? expand(node.type(node.props)) : [node, ...expand(node.props?.children)])
    : typeof node === 'string' ? [node] : [];
const text = nodes => nodes.filter(node => typeof node === 'string').join(' ');

function setup(t, { paused = false, reducedMotion = false } = {}) {
  const hooks = createHookHarness();
  const timers = [];
  const previous = { setInterval: globalThis.setInterval, clearInterval: globalThis.clearInterval, window: globalThis.window };
  globalThis.setInterval = (callback, delay) => { timers.push({ callback, delay }); return timers.length; };
  globalThis.clearInterval = id => { if (timers[id - 1]) timers[id - 1] = { callback: noop, delay: 0, cleared: true }; };
  globalThis.window = { matchMedia: query => ({ matches: reducedMotion && query.includes('reduce'), addEventListener: noop, removeEventListener: noop }) };
  t.after(() => { Object.assign(globalThis, previous); hooks.unmount(); });
  let handler;
  const subscriptions = [];
  const meter = loadTsModule('src/components/AudioLevelMeter.tsx', { react: hooks.react });
  const levelsLib = loadTsModule('src/lib/recording-levels.ts', {
    '@/components/AudioLevelMeter': meter,
    '@tauri-apps/api/core': { invoke: async () => {} },
    '@tauri-apps/api/event': { listen: async () => noop },
  });
  const { RecordingLevelMeter } = loadTsModule('src/components/RecordingLevelMeter.tsx', {
    react: hooks.react,
    'react/jsx-runtime': { jsx, jsxs: jsx },
    '@tauri-apps/api/event': { listen: async (name, callback) => { subscriptions.push(name); handler = callback; return noop; } },
    '@/components/AudioLevelMeter': { SegmentedLevelMeter: 'meter' },
    '@/lib/recording-levels': levelsLib,
  });
  let isPaused = paused;
  const render = () => hooks.render(() => RecordingLevelMeter({ paused: isPaused }));
  render();
  return {
    render, subscriptions,
    send: payload => handler({ payload }),
    tick: () => timers.filter(timer => !timer.cleared).at(-1)?.callback(),
    timers: () => timers.filter(timer => !timer.cleared),
    pause: value => { isPaused = value; },
    rows: () => expand(render()),
  };
}

const meters = nodes => nodes.filter(node => node && node.type === 'meter');

test('it subscribes to the recording levels once', async t => {
  const app = setup(t);
  await flush();
  assert.deepEqual(app.subscriptions, ['recording-levels']);
  app.send({ mic: { rms: 0.1, peak: 0.3 }, system: null });
  app.render();
  assert.deepEqual(app.subscriptions, ['recording-levels'], 'rendering does not subscribe again');
});

test('the microphone is "Me" and system audio is "Meeting audio", each with its own meter', async t => {
  const app = setup(t);
  await flush();
  app.send({ mic: { rms: 0.1, peak: 0.3 }, system: { rms: 0.01, peak: 0.02 } });
  const nodes = app.rows();
  const drawn = meters(nodes);
  assert.deepEqual(drawn.map(node => node.props.label), ['Me level', 'Meeting audio level']);
  assert.ok(Math.abs(drawn[0].props.db - -20) < 0.01);
  assert.ok(Math.abs(drawn[1].props.db - -40) < 0.01);
  assert.match(text(nodes), /Me\b/);
  assert.match(text(nodes), /Meeting audio/);
});

test('a source that is not captured says so instead of drawing silence', async t => {
  const app = setup(t);
  await flush();
  app.send({ mic: { rms: 0.1, peak: 0.3 }, system: null });
  const nodes = app.rows();
  assert.equal(meters(nodes).length, 1, 'only the captured source has a meter');
  assert.match(text(nodes), /Not captured/);
});

test('without events for half a second the meters read as silence', async t => {
  const app = setup(t);
  await flush();
  app.send({ mic: { rms: 0.1, peak: 0.3 }, system: { rms: 0.1, peak: 0.3 } });
  await new Promise(resolve => setTimeout(resolve, 650));
  app.tick();
  const drawn = meters(app.rows());
  assert.deepEqual(drawn.map(node => node.props.db), [-60, -60]);
});

test('it is labelled as a group and announces only state changes, never the levels', async t => {
  const app = setup(t);
  await flush();
  app.send({ mic: { rms: 0.1, peak: 0.3 }, system: null });
  const tree = app.render();
  assert.equal(tree.props.role, 'group');
  assert.equal(tree.props['aria-label'], 'Recording levels');
  const nodes = expand(tree);
  const live = nodes.filter(node => node && node.props && (node.props.role === 'status' || node.props['aria-live']));
  assert.equal(live.length, 1);
  assert.equal(text(expand(live[0].props.children)), '', 'quiet while levels change');
});

test('paused rests the meters and says so; events that still arrive are ignored', async t => {
  const app = setup(t);
  await flush();
  app.send({ mic: { rms: 0.1, peak: 0.3 }, system: { rms: 0.1, peak: 0.3 } });
  app.pause(true);
  app.render();
  app.send({ mic: { rms: 0.5, peak: 0.9 }, system: { rms: 0.5, peak: 0.9 } });
  const nodes = app.rows();
  assert.deepEqual(meters(nodes).map(node => node.props.db), [null, null]);
  assert.match(text(nodes), /Paused/);
  assert.match(text(nodes), /Recording paused/, 'announced once for screen readers');
  assert.equal(app.timers().length, 0, 'no timers run while paused');
});

test('a microphone that has made no sound for a long stretch is noted quietly in its row', async t => {
  const app = setup(t);
  await flush();
  // Zero readings spread over 10 s, using the real clock for the first and a faked later one.
  const realNow = Date.now;
  let now = realNow();
  Date.now = () => now;
  try {
    app.send({ mic: { rms: 0, peak: 0 }, system: { rms: 0.1, peak: 0.2 } });
    now += 10_000;
    app.send({ mic: { rms: 0, peak: 0 }, system: { rms: 0.1, peak: 0.2 } });
  } finally { Date.now = realNow; }
  const nodes = app.rows();
  assert.match(text(nodes), /No microphone signal yet/);
});

test('with reduced motion the meters refresh at the slower pace', async t => {
  const app = setup(t, { reducedMotion: true });
  await flush();
  app.render();
  assert.equal(app.timers().at(-1).delay, 500);
  app.send({ mic: { rms: 0.1, peak: 0.3 }, system: null });
  assert.deepEqual(meters(app.rows()).map(node => node.props.db), [null, null], 'the reading waits for the next refresh');
  app.tick();
  const refreshed = meters(app.rows());
  assert.equal(refreshed.length, 1);
  assert.ok(Math.abs(refreshed[0].props.db - -20) < 0.01);
  const normal = setup(t);
  await flush();
  assert.equal(normal.timers().at(-1).delay, 200);
});

test('a microphone that was heard and then goes quiet is not flagged', async t => {
  const app = setup(t);
  await flush();
  const realNow = Date.now;
  let now = realNow();
  Date.now = () => now;
  try {
    app.send({ mic: { rms: 0.1, peak: 0.3 }, system: { rms: 0.1, peak: 0.2 } });
    for (let i = 0; i < 4; i++) {
      now += 30_000;
      app.send({ mic: { rms: 0, peak: 0 }, system: { rms: 0.1, peak: 0.2 } });
    }
  } finally { Date.now = realNow; }
  assert.doesNotMatch(text(app.rows()), /No microphone signal/);
});
