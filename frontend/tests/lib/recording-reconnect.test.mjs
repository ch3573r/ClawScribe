import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

test('failed loopback keeps Retry while the microphone continues', async () => {
  const hooks = createHookHarness();
  const previousWindow = globalThis.window;
  globalThis.window = { setInterval: () => 1, clearInterval: () => {} };
  const attempts = [];
  const warnings = [];
  const successes = [];
  const noop = () => {};
  const { AudioDeviceHotSwapMonitor } = loadTsModule('src/components/AudioDeviceHotSwapMonitor.tsx', {
    react: hooks.react,
    sonner: { toast: {
      warning: (title, options) => warnings.push({ title, ...options }),
      success: title => successes.push(title), loading: noop, dismiss: noop,
    } },
    '@/contexts/RecordingStateContext': { useRecordingState: () => ({ isRecording: true, sessionId: 'producer' }) },
    '@/services/recordingService': { recordingService: {
      pollAudioDeviceEvents: async () => null,
      getReconnectionStatus: async () => ({ is_reconnecting: false, disconnected_devices: [
        { name: 'Synthetic output', device_type: 'SystemAudio' },
      ] }),
      attemptDeviceReconnect: async (...args) => {
        attempts.push(args);
        return { reconnected: attempts.length === 2, microphone_active: true, system_audio_active: attempts.length === 2 };
      },
    } },
  });
  try {
    hooks.render(AudioDeviceHotSwapMonitor);
    await flush();
    warnings.at(-1).action.onClick();
    await flush();
    assert.deepEqual(successes, []);
    assert.match(warnings.at(-1).description, /Microphone capture continues/);
    warnings.at(-1).action.onClick();
    await flush();
    assert.deepEqual(successes, ['System audio reconnected']);
    assert.deepEqual(attempts, Array(2).fill(['Synthetic output', 'SystemAudio', 'producer']));
  } finally {
    hooks.unmount();
    globalThis.window = previousWindow;
  }
});
