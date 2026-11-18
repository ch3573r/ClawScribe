import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const jsx = (type, props) => ({ type, props });
function nodes(root) {
  if (Array.isArray(root)) return root.flatMap(nodes);
  return root?.props ? [root, ...nodes(root.props.children)] : [];
}
function text(root) {
  if (Array.isArray(root)) return root.map(text).join('');
  return root?.props ? text(root.props.children) : String(root ?? '');
}

function setup(t, provider, model) {
  const hooks = createHookHarness();
  t.after(() => hooks.unmount());
  const calls = [], modes = [], run = deferred();
  const { TranscriptionPcTest } = loadTsModule('src/components/TranscriptionPcTest.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/components/ui/button': { Button: 'Button' }, '@/components/ui/page-section': { PageSection: 'PageSection' },
    '@/contexts/RecordingStateContext': { useRecordingState: () => ({ setRecordingMode: async mode => modes.push(mode) }) },
    '@/lib/parakeet': { getModelDisplayName: () => 'Lightning' },
    '@tauri-apps/api/core': { invoke: (name, args) => { calls.push([name, args]); return run.promise; } },
  });
  const render = () => hooks.render(() => TranscriptionPcTest({ provider, model }));
  const button = label => nodes(render()).find(node => node.type === 'Button' && text(node) === label);
  return { render, button, calls, modes, run };
}

test('hosted selections benchmark only their local model and offer audio-only after a slow or incomplete run', async t => {
  const { render, button, calls, modes, run } = setup(t, 'cloud-whisper', 'cloud-model');
  const start = button('Test this PC').props.onClick;
  start(); start();
  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], 'run_transcription_pc_test');
  assert.equal(calls[0][1].provider, 'parakeet');
  assert.equal(calls[0][1].model, 'parakeet-tdt-0.6b-v3-int8');
  assert.equal(button('Testing this PC…').props.disabled, true);
  run.resolve({ engine: 'Parakeet', model: calls[0][1].model, backend: 'CPU', audioSeconds: 30,
    elapsedSeconds: 38, realtimeFactor: 38 / 30, fallbackReason: 'The worker switched to CPU.',
    samples: [{ language: 'de', audioSeconds: 15, elapsedSeconds: 20, recognized: false },
      { language: 'en', audioSeconds: 15, elapsedSeconds: 18, recognized: true }] });
  await flush();
  assert.match(text(render()), /missed speech/);
  assert.match(text(render()), /fell behind/);
  assert.match(text(render()), /38\.0 s for 30\.0 s/);
  assert.match(text(render()), /worker switched to CPU/);
  button('Use record now, transcribe later').props.onClick();
  assert.deepEqual(modes, ['audio_only']);
});

test('both installed Nemotron variants use the selected engine and model', async t => {
  for (const variant of ['fp16', 'int8']) {
    const model = 'nemotron-streaming-0.6b-' + variant;
    const { render, button, calls, run } = setup(t, 'nemotron', model);
    assert.match(text(render()), new RegExp('Nemotron · ' + variant.toUpperCase()));
    assert.equal(button('Test this PC').props.disabled, false);
    button('Test this PC').props.onClick();
    assert.deepEqual(structuredClone(calls), [['run_transcription_pc_test', { provider: 'nemotron', model }]]);
    run.resolve({ engine: 'Nemotron', model, backend: 'CPU', audioSeconds: 30, elapsedSeconds: 14,
      realtimeFactor: 14 / 30, samples: [] });
    await flush();
    assert.match(text(render()), /kept up/);
  }
});
