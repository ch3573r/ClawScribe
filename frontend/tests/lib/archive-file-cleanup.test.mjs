import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const jsx = (type, props) => ({ type, props });

function find(tree, predicate) {
  if (Array.isArray(tree)) return tree.map(child => find(child, predicate)).find(Boolean);
  if (!tree || typeof tree !== 'object') return undefined;
  return predicate(tree) ? tree : find(tree.props?.children, predicate);
}

test('storage reviews an exact cleanup entry and keeps it visible when retry is refused', async t => {
  const hooks = createHookHarness();
  let open;
  let pending = [{ id: 18, title: 'Synthetic meeting', kind: 'recording', location: 'Example recording folder', reason: 'Folder is in use.' }];
  const calls = [];
  const previousDocument = globalThis.document;
  globalThis.document = { activeElement: null };
  t.after(() => { hooks.unmount(); globalThis.document = previousDocument; });
  const { ArchiveStorageDialog } = loadTsModule('src/components/Archive/ArchiveStorageDialog.tsx', {
    react: hooks.react,
    'react/jsx-runtime': { jsx, jsxs: jsx, Fragment: 'Fragment' },
    '@/components/ui/button': { Button: 'Button' },
    '@/components/ui/dialog': Object.fromEntries(['Dialog', 'DialogContent', 'DialogDescription', 'DialogFooter', 'DialogHeader', 'DialogTitle'].map(name => [name, name])),
    '@/lib/storage-dialog-controller': { registerStorageDialogOpener(callback) { open = callback; } },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push([command, args?.id]);
      if (command === 'list_pending_meeting_file_cleanup') return pending;
      if (command === 'measure_archive_storage') throw new Error('Synthetic measurement failure');
      if (command === 'retry_meeting_file_cleanup') return 'A saved meeting still uses the folder.';
      if (command === 'keep_pending_meeting_files') { pending = []; return; }
      assert.fail(`Unexpected command ${command}`);
    } },
  });
  const render = () => hooks.render(ArchiveStorageDialog);
  const button = (tree, label) => find(tree, node => node.type === 'Button' && node.props.children === label);
  const showsText = (tree, text) => Boolean(find(tree, node => node.props?.children === text));
  render();
  open();
  render();
  await flush();
  const list = render();
  assert.ok(showsText(list, 'Could not measure library storage.'));
  button(list, 'Review').props.onClick();
  const review = render();
  assert.ok(showsText(review, 'Example recording folder'));
  assert.ok(showsText(review, 'Folder is in use.'));
  assert.equal(calls.filter(([command]) => command === 'retry_meeting_file_cleanup').length, 0);

  button(review, 'Retry cleanup').props.onClick();
  assert.equal(button(render(), 'Retry cleanup').props.disabled, true);
  await flush();
  const refused = render();
  assert.ok(showsText(refused, 'A saved meeting still uses the folder.'));
  assert.deepEqual(calls.at(-1), ['retry_meeting_file_cleanup', 18]);

  button(refused, 'Keep files').props.onClick();
  await flush();
  assert.deepEqual(calls.at(-1), ['keep_pending_meeting_files', 18]);
  render();
  await flush();
  const kept = render();
  assert.equal(button(kept, 'Review'), undefined);
  assert.ok(showsText(kept, 'Files kept and removed from the cleanup list.'));
});
