import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const remote = {
  provider: 'ollama', model: 'synthetic', endpoint: 'https://models.example.com/api',
  external: true, unencrypted: false, fingerprint: 'reviewed', approved: true,
};

test('imports inherit an eligible approval and keep opt out and one-file approval independent', () => {
  const { importSummarySelection } = loadTsModule('src/hooks/useSummaryDestination.ts');
  assert.equal(importSummarySelection('default', true, remote).useAutomaticPreference, true);
  assert.equal(importSummarySelection('default', true, { ...remote, approved: false }), null);
  assert.equal(importSummarySelection('default', false, remote), null);
  assert.equal(importSummarySelection('off', true, remote), null);

  const local = { ...remote, external: false, approved: false };
  const oneFile = { fingerprint: local.fingerprint };
  assert.equal(importSummarySelection(oneFile, false, local).useAutomaticPreference, false);
  assert.equal(importSummarySelection(oneFile, false, { ...remote, fingerprint: 'different' }), null);
  assert.equal(importSummarySelection(oneFile, false, null), null);
});

test('destination refresh discards the earlier review and removes its event listener', async () => {
  const hooks = createHookHarness();
  const earlier = deferred();
  const newer = deferred();
  const calls = [];
  let onChange;
  let unlistened = false;
  const { useSummaryDestination } = loadTsModule('src/hooks/useSummaryDestination.ts', {
    react: hooks.react,
    '@tauri-apps/api/core': { invoke: (command, args) => {
      calls.push([command, args]);
      return (calls.length === 1 ? earlier : newer).promise;
    } },
    '@tauri-apps/api/event': { listen: async (event, listener) => {
      assert.equal(event, 'summary-destination-changed');
      onChange = listener;
      return () => { unlistened = true; };
    } },
  });
  const config = { provider: 'ollama', model: 'synthetic' };
  const render = () => hooks.render(() => useSummaryDestination(true, config));
  render();
  onChange();
  render();
  newer.resolve({ ...remote, fingerprint: 'changed', approved: false });
  await flush();
  earlier.resolve(remote);
  await flush();
  assert.equal(render().destination.fingerprint, 'changed');
  assert.equal(render().destination.approved, false);
  assert.deepEqual(calls, Array.from({ length: 2 }, () => ['get_automatic_summary_destination', undefined]));
  hooks.unmount();
  await flush();
  assert.equal(unlistened, true);
});
