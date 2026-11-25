import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const jsx = (type, props) => ({ type, props });
const nodes = node => Array.isArray(node) ? node.flatMap(nodes)
  : node?.props ? [node, ...nodes(node.props.children)] : [];
const copy = node => Array.isArray(node) ? node.map(copy).join('')
  : typeof node === 'string' ? node : node?.props ? copy(node.props.children) : '';

test('the first scheduled backup can exclude audio and previews the selected size', async t => {
  const hooks = createHookHarness();
  t.after(hooks.unmount);
  const calls = [];
  let status = { enabled: false, destination: null, exclude_audio: false };
  const { ScheduledBackups } = loadTsModule('src/components/ScheduledBackups.tsx', {
    react: { ...hooks.react, useId: () => 'field' },
    'react/jsx-runtime': { jsx, jsxs: jsx, Fragment: 'fragment' },
    '@/components/ui/button': { Button: 'button' },
    '@/components/ui/switch': { Switch: 'switch' },
    '@tauri-apps/api/event': { listen: async () => () => {} },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push([command, args]);
      if (command === 'scheduled_backup_status') return status;
      if (command === 'measure_archive_storage') {
        return { saved_audio_bytes: 65 * 1024, database_bytes: 1024, reference_bytes: 1024, partial: false, unavailable_folders: 0 };
      }
      assert.equal(command, 'choose_scheduled_backup_folder');
      status = { enabled: true, destination: 'Backups', exclude_audio: args.excludeAudio };
    } },
  });
  const render = () => hooks.render(() => ScheduledBackups({ disabled: false }));
  render(); await flush();
  assert.match(copy(render()), /Estimated size per backup.*67 KiB/);
  nodes(render()).find(node => node.type === 'switch').props.onCheckedChange(true);
  assert.match(copy(render()), /Estimated size per backup.*2 KiB/);
  await nodes(render()).find(node => node.type === 'button').props.onClick();
  await flush();
  assert.equal(calls.find(([command]) => command === 'choose_scheduled_backup_folder')[1].excludeAudio, true);
  assert.match(copy(render()), /Backups/);
});
