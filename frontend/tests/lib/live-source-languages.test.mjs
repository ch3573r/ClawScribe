import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const jsx = (type, props) => ({ type, props });
function text(node) {
  if (Array.isArray(node)) return node.map(text).join('');
  return node?.props ? text(node.props.children) : String(node ?? '');
}

test('source languages reject stale snapshots and events from a previous recording', async () => {
  const hooks = createHookHarness();
  const read = deferred();
  let emit, unsubscribed = false;
  const { LiveSourceLanguages } = loadTsModule('src/components/LiveSourceLanguages.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@tauri-apps/api/core': { invoke: () => read.promise },
    '@tauri-apps/api/event': { listen: async (_name, cb) => { emit = value => cb({ payload: value }); return () => { unsubscribed = true; }; } },
  });
  const render = () => hooks.render(() => LiveSourceLanguages({ sessionId: 'current' }));
  const observed = { session_id: 'current', revision: 3, microphone: 'de', system: 'en', evidence: 'whisper_audio' };
  assert.equal(render(), null);
  await flush();
  emit(observed);
  emit({ ...observed, session_id: 'previous', revision: 99, microphone: 'fr' });
  read.resolve({ ...observed, revision: 2, system: null });
  await flush();
  assert.match(text(render()), /Microphone · German/);
  assert.match(text(render()), /System audio · English/);
  emit({ ...observed, revision: 4, microphone: null, system: null });
  assert.equal(render(), null);
  hooks.unmount();
  await flush();
  assert.equal(unsubscribed, true);
});
