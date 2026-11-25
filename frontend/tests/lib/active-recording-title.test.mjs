import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const jsx = (type, props) => ({ type, props });
const nodes = node => Array.isArray(node) ? node.flatMap(nodes) : node?.props ? [node, ...nodes(node.props.children)] : [];

test('live title edits target the active session, survive a refusal, and submit just once', async t => {
  const hooks = createHookHarness();
  t.after(() => hooks.unmount());
  const attempts = [];
  const saved = deferred();
  const names = ['Dialog', 'DialogContent', 'DialogDescription', 'DialogFooter', 'DialogHeader', 'DialogTitle', 'DialogTrigger'];
  const { ActiveRecordingTitle } = loadTsModule('src/app/_components/ActiveRecordingTitle.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx }, 'lucide-react': {},
    '@/components/ui/button': { Button: 'Button' },
    '@/components/ui/input': { Input: 'Input' },
    '@/components/ui/dialog': Object.fromEntries(names.map(name => [name, name])),
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      assert.equal(args.sessionId, 'active-recording');
      if (command === 'get_active_recording_title') return 'Current meeting';
      assert.equal(command, 'set_active_recording_title');
      attempts.push(args.title);
      if (attempts.length === 1) return saved.promise;
    } },
  });
  const render = () => hooks.render(() => ActiveRecordingTitle({ sessionId: 'active-recording' }));
  const find = type => nodes(render()).find(node => node.type === type);
  render().props.onOpenChange(true);
  await flush();
  assert.equal(find('Input').props.value, 'Current meeting');
  find('Input').props.onChange({ target: { value: 'Edited title' } });
  const submit = find('form').props.onSubmit;
  const first = submit({ preventDefault() {} });
  await submit({ preventDefault() {} });
  assert.deepEqual(attempts, ['Edited title']);
  saved.reject('Synthetic write refusal');
  await first;
  assert.equal(find('Input').props.value, 'Edited title');
  assert.ok(nodes(render()).some(node => node.props.role === 'alert'));
  await find('form').props.onSubmit({ preventDefault() {} });
  assert.equal(render().props.open, false);
});
