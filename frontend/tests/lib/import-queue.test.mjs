import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

test('closing the queue leaves the native job alone and reopening reloads its saved result', async () => {
  const harness = createHookHarness();
  const requested = [];
  const handlers = new Map();
  const requests = [];
  const timers = new Map();
  const previousWindow = globalThis.window;
  let handle = 0;
  globalThis.window = {
    setInterval: callback => { timers.set(++handle, callback); return handle; },
    clearInterval: id => timers.delete(id),
  };
  try {
    const { useImportQueue } = loadTsModule('src/hooks/useImportAudio.ts', {
      react: harness.react,
      '@tauri-apps/api/core': { invoke: command => {
        requested.push(command);
        const request = deferred();
        requests.push(request);
        return request.promise;
      } },
      '@tauri-apps/api/event': { listen: async (name, callback) => {
        handlers.set(name, callback);
        return () => handlers.delete(name);
      } },
    });
    const render = open => harness.render(() => useImportQueue(open));
    render(true);
    await flush();
    handlers.get('import-queue-changed')();
    [...timers.values()][0]();
    assert.equal(requested.length, 1, 'updates coalesce during a slow snapshot');
    requests[0].resolve([{ id: 'meeting-a', status: 'running' }]);
    await flush();
    assert.equal(render(true).items[0].status, 'running');
    assert.equal(requested.length, 2);

    render(false);
    requests[1].resolve([{ id: 'meeting-a', status: 'failed' }]);
    await flush();
    assert.equal(handlers.size, 0);
    assert.equal(timers.size, 0);
    assert.equal(render(false).items[0].status, 'running', 'closed dialog ignores late updates');
    render(true);
    requests[2].resolve([{ id: 'meeting-a', status: 'saved', result: { meeting_id: 'meeting-a', files_pending: true } }]);
    await flush();
    assert.equal(render(true).items[0].result.files_pending, true);
    assert.ok(requested.every(command => command === 'import_queue_snapshot'));
  } finally {
    harness.unmount();
    globalThis.window = previousWindow;
  }
});
