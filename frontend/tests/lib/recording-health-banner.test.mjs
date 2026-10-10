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

function createWarningView() {
  const hooks = createHookHarness();
  const listeners = new Map();
  const { RecordingHealthBanner } = loadTsModule(fileURLToPath(new URL('../../src/components/RecordingHealthBanner.tsx', import.meta.url)), {
    react: hooks.react,
    '@tauri-apps/api/event': { listen: async (event, callback) => { listeners.set(event, callback); return () => listeners.delete(event); } },
    'lucide-react': { AlertTriangle: 'Icon' },
  });
  return { ...hooks, listeners, render: () => hooks.render(RecordingHealthBanner) };
}

test('late system audio clears the matching recording warning', () => {
  const view = createWarningView();
  const systemWarning = 'No system-audio samples have arrived yet.';
  try {
    view.render();
    view.listeners.get('recording-warning')({ payload: systemWarning });
    assert.equal(view.render().props.children[1].props.children, systemWarning);
    view.listeners.get('recording-warning-cleared')?.({ payload: systemWarning });
    assert.equal(view.render(), null, 'Recovered system audio must remove its stale warning');
  } finally {
    view.unmount();
  }
});

test('system audio recovery cannot hide a newer capture warning', () => {
  const view = createWarningView();
  const systemWarning = 'No system-audio samples have arrived yet.';
  const captureWarning = 'Some audio could not be saved.';
  try {
    view.render();
    view.listeners.get('recording-warning')({ payload: systemWarning });
    view.listeners.get('recording-warning')({ payload: captureWarning });
    view.listeners.get('recording-warning-cleared')?.({ payload: systemWarning });
    assert.equal(view.render().props.children[1].props.children, captureWarning);
  } finally {
    view.unmount();
  }
});
