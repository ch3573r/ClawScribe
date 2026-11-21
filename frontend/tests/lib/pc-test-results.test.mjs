import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

function setup(t, initial) {
  const hooks = createHookHarness();
  const store = new Map(initial === undefined ? [] : [['clawscribe.transcriptionPcTest.v1', initial]]);
  const listeners = new Map();
  const previous = { window: globalThis.window, CustomEvent: globalThis.CustomEvent };
  globalThis.CustomEvent = class { constructor(type) { this.type = type; } };
  globalThis.window = {
    localStorage: {
      getItem: key => store.get(key) ?? null,
      setItem: (key, value) => store.set(key, value),
    },
    addEventListener: (type, callback) => listeners.set(type, callback),
    removeEventListener: type => listeners.delete(type),
    dispatchEvent: event => listeners.get(event.type)?.(event),
  };
  t.after(() => { hooks.unmount(); Object.assign(globalThis, previous); if (!previous.window) delete globalThis.window; });
  const module = loadTsModule('src/components/settings/pcTestResults.ts', { react: hooks.react });
  return { ...module, hooks, store };
}

const measurement = { backend: 'CPU', realtimeFactor: 0.05, samples: [{ recognized: true }, { recognized: true }] };

test('a finished test is kept per model id and read back as a real-time multiple', t => {
  const m = setup(t);
  assert.equal(Object.keys(m.readPcTestResults()).length, 0);
  assert.equal(m.savePcTestResult('parakeet-tdt-0.6b-v3-int8', measurement, 1000), true);
  m.savePcTestResult('parakeet-tdt-0.6b-ultra-int8', { ...measurement, realtimeFactor: 0.1, samples: [{ recognized: false }] }, 2000);
  const results = m.readPcTestResults();
  assert.equal(Object.keys(results).length, 2);
  assert.equal(results['parakeet-tdt-0.6b-v3-int8'].measuredAt, 1000);
  assert.equal(results['parakeet-tdt-0.6b-v3-int8'].backend, 'CPU');
  assert.equal(results['parakeet-tdt-0.6b-v3-int8'].missedSpeech, false);
  assert.equal(results['parakeet-tdt-0.6b-ultra-int8'].missedSpeech, true);
  assert.equal(Math.round(m.speedOf(results['parakeet-tdt-0.6b-v3-int8'])), 20);
  assert.equal(m.fastestSpeed(Object.values(results)), 20);
});

test('unusable measurements and corrupt storage are ignored instead of shown', t => {
  const m = setup(t, '{"a":{"realtimeFactor":0,"measuredAt":1},"b":{"realtimeFactor":"fast","measuredAt":1},"c":{"realtimeFactor":0.5,"measuredAt":5}}');
  assert.deepEqual([...Object.keys(m.readPcTestResults())], ['c']);
  assert.equal(m.savePcTestResult('x', { ...measurement, realtimeFactor: 0 }), false);
  assert.equal(m.savePcTestResult('x', { ...measurement, realtimeFactor: Number.NaN }), false);
  const broken = setup(t, '{not json');
  assert.equal(Object.keys(broken.readPcTestResults()).length, 0);
});

test('speed text, bar scale and the time of the last test', t => {
  const m = setup(t);
  assert.equal(m.formatSpeed(21.6), '22× real time');
  assert.equal(m.formatSpeed(4.26), '4.3× real time');
  assert.equal(m.speedFraction(10, 20), 0.5);
  assert.equal(m.speedFraction(20, 20), 1);
  assert.equal(m.speedFraction(0.1, 20), 0.04, 'a slow model still gets a visible bar');
  assert.equal(m.speedFraction(5, 0), 0);
  const now = new Date(2026, 10, 19, 12, 0).getTime();
  assert.equal(m.formatMeasuredAt(new Date(2026, 10, 19, 10, 31).getTime(), now), 'today 10:31');
  assert.equal(m.formatMeasuredAt(new Date(2026, 10, 18, 9, 5).getTime(), now), 'yesterday 09:05');
  assert.equal(m.formatMeasuredAt(new Date(2026, 10, 3, 9, 5).getTime(), now), '3 Nov 09:05');
});

test('the results hook follows a test that finishes elsewhere in the app', t => {
  const m = setup(t);
  const render = () => m.hooks.render(() => m.usePcTestResults());
  render();
  assert.equal(Object.keys(render()).length, 0);
  m.savePcTestResult('model-a', measurement, 1);
  assert.deepEqual([...Object.keys(render())], ['model-a']);
});

test('a speed recommendation requires two installed models recognizing both samples on the same known backend', t => {
  const m = setup(t);
  const ids = ['quick', 'other', 'missed', 'deleted', 'untested', 'gpu', 'incomplete', 'legacy'];
  const models = ids.map(name => ({ name, status: name === 'deleted' ? 'Missing' : 'Available' }));
  const recommend = () => [...m.recommendedForSpeed(models, m.readPcTestResults())];
  m.savePcTestResult('quick', measurement);
  m.savePcTestResult('missed', { ...measurement, realtimeFactor: 0.01, samples: [{ recognized: false }, { recognized: true }] });
  m.savePcTestResult('deleted', measurement);
  m.savePcTestResult('gpu', { ...measurement, backend: 'DirectML + CPU' });
  m.savePcTestResult('incomplete', { ...measurement, samples: [] });
  assert.deepEqual(recommend(), []);
  m.savePcTestResult('other', measurement);
  assert.deepEqual(recommend(), [], 'a tie has no single winner');
  m.savePcTestResult('other', { ...measurement, realtimeFactor: 0.2 });
  assert.deepEqual(structuredClone(recommend()), [['quick', 'Fastest of 2 tested on CPU']]);
  for (const backend of ['Unconfirmed', 'CPU / Vulkan', '']) {
    for (const id of ['quick', 'other']) m.savePcTestResult(id, { ...measurement, backend, realtimeFactor: id === 'quick' ? 0.1 : 0.2 });
    assert.deepEqual(recommend(), []);
  }
});
