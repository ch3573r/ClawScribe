import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness } from './hook-harness.mjs';

test('repeated segment warnings keep the recording banner visible', () => {
  const hooks = createHookHarness();
  const listeners = new Map();
  const { RecordingHealthBanner } = loadTsModule(fileURLToPath(new URL('../../src/components/RecordingHealthBanner.tsx', import.meta.url)), {
    react: hooks.react,
    '@tauri-apps/api/event': { listen: async (event, callback) => { listeners.set(event, callback); return () => listeners.delete(event); } },
    'lucide-react': { AlertTriangle: 'Icon' },
  });
  assert.equal(hooks.render(RecordingHealthBanner), null);
  listeners.get('transcription-error')({ payload: {} });
  for (let i = 0; i < 10; i++) listeners.get('transcription-warning')({ payload: 'Synthetic failure' });
  const banner = hooks.render(RecordingHealthBanner);
  assert.equal(banner.props.role, 'alert');
  assert.match(banner.props.children[1].props.children, /Live transcription is incomplete/);
  listeners.get('recording-started')({});
  assert.equal(hooks.render(RecordingHealthBanner), null);
  hooks.unmount();
});
