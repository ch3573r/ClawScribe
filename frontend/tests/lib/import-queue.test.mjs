import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const jsx = (type, props) => ({ type, props });
const nodes = node => Array.isArray(node) ? node.flatMap(nodes) : node?.props ? [node, ...nodes(node.props.children)] : [];

test('Sidebar shows active import progress with the dialog closed and offers detection for the saved meeting', t => {
  const hooks = createHookHarness();
  t.after(() => hooks.unmount());
  const result = { meeting_id: 'imported', segments_count: 3, files_pending: false };
  const active = { id: 'job', title: 'Imported call', status: 'running', progress: { progress_percentage: 38 } };
  let items = [active];
  const { SidebarImportStatus } = loadTsModule('src/components/Sidebar/SidebarImportStatus.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx }, 'lucide-react': {},
    'next/navigation': { useRouter: () => ({}) },
    '@/components/ui/button': { Button: 'Button' },
    '@/components/ui/progress': { Progress: 'Progress' },
    '@/components/ui/tooltip': {},
    '@/contexts/ImportDialogContext': { useBackgroundImportQueue: () => ({ items }), useImportDialog: () => ({}) },
    '@/components/ImportAudio/ImportSpeakerDetectionButton': { ImportSpeakerDetectionButton: 'Detect' },
  });
  const find = type => nodes(hooks.render(() => SidebarImportStatus({ collapsed: false }))).find(node => node.type === type);
  assert.equal(find('Progress').props.value, 38);
  assert.equal(find('Detect'), undefined);
  items = [{ ...active, status: 'saved', progress: null, result }];
  assert.equal(find('Progress'), undefined);
  assert.equal(find('Detect').props.result.meeting_id, 'imported');
});

test('speaker detection for an imported meeting is explicitly submitted once with its saved folder', async t => {
  const hooks = createHookHarness();
  t.after(() => hooks.unmount());
  const native = [];
  const pending = deferred();
  const { ImportDialogProvider } = loadTsModule('src/contexts/ImportDialogContext.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@/hooks/useImportAudio': { useImportQueue: () => ({}) },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      native.push([command, args]);
      if (command === 'api_get_meeting_metadata') return { folder_path: 'synthetic saved folder' };
      return pending.promise;
    } },
  });
  const queue = () => hooks.render(() => ImportDialogProvider({})).props.children.props.value;
  const action = queue().detectSpeakers;
  const run = action('imported');
  assert.equal(await action('imported'), false);
  assert.equal(queue().detecting, 'imported');
  assert.deepEqual(native.map(([command, args]) => [command, args.meetingId]), [
    ['api_get_meeting_metadata', 'imported'], ['start_speaker_diarization_command', 'imported'],
  ]);
  assert.equal(native[1][1].meetingFolderPath, 'synthetic saved folder');
  assert.equal(native[1][1].preserveExistingLabels, true);
  pending.resolve();
  assert.equal(await run, true);
  assert.ok(queue().detected.has('imported'));
});

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
    requests[0].resolve({ items: [{ id: 'meeting-a', status: 'running' }], recovery_notice: null });
    await flush();
    assert.equal(render(true).items[0].status, 'running');
    assert.equal(requested.length, 2);

    render(false);
    requests[1].resolve({ items: [{ id: 'meeting-a', status: 'failed' }], recovery_notice: null });
    await flush();
    assert.equal(handlers.size, 0);
    assert.equal(timers.size, 0);
    assert.equal(render(false).items[0].status, 'running', 'closed dialog ignores late updates');
    render(true);
    const notice = 'The unreadable import queue was preserved. You can choose files again.';
    requests[2].resolve({ items: [{ id: 'meeting-a', status: 'saved', result: { meeting_id: 'meeting-a', files_pending: true } }], recovery_notice: notice });
    await flush();
    assert.equal(render(true).items[0].result.files_pending, true);
    assert.equal(render(true).recoveryNotice, notice);
    assert.ok(requested.every(command => command === 'import_queue_snapshot'));
  } finally {
    harness.unmount();
    globalThis.window = previousWindow;
  }
});
