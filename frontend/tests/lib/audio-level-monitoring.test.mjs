import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, flush } from './hook-harness.mjs';

function elements(value) {
  if (Array.isArray(value)) return value.flatMap(elements);
  if (!value || typeof value !== 'object') return [];
  return [value, ...elements(value.props?.children)];
}

function createView({ selectedMic, monitoredDevices }) {
  const hooks = createHookHarness();
  const listeners = new Map();
  const devices = [
    { name: 'Microphone (2- USB Headset)', device_type: 'Input' },
    { name: 'Built-in Microphone', device_type: 'Input' },
    { name: 'Desk Speakers', device_type: 'Output' },
  ];
  const { DeviceSelection } = loadTsModule(
    fileURLToPath(new URL('../../src/components/DeviceSelection.tsx', import.meta.url)),
    {
      react: hooks.react,
      'react/jsx-runtime': {
        jsx: (type, props) => ({ type, props }),
        jsxs: (type, props) => ({ type, props }),
      },
      '@tauri-apps/api/core': {
        invoke: async command => {
          if (command === 'get_audio_devices') return devices;
          if (command === 'start_audio_level_monitoring') return monitoredDevices;
          if (command === 'stop_audio_level_monitoring') return;
          throw new Error(`Unexpected command: ${command}`);
        },
      },
      '@tauri-apps/api/event': {
        listen: async (event, callback) => {
          listeners.set(event, callback);
          return () => listeners.delete(event);
        },
      },
      'lucide-react': { RefreshCw: 'Icon', Mic: 'Icon', Speaker: 'Icon' },
      './AudioLevelMeter': { AudioLevelMeter: 'AudioLevelMeter', CompactAudioLevelMeter: 'CompactAudioLevelMeter' },
      './AudioBackendSelector': { AudioBackendSelector: 'AudioBackendSelector' },
      '@/components/ui/select': Object.fromEntries(
        ['Select', 'SelectContent', 'SelectItem', 'SelectTrigger', 'SelectValue'].map(name => [name, name]),
      ),
      '@/components/ui/label': { Label: 'Label' },
      '@/components/ui/button': { Button: 'Button' },
      '@/lib/analytics': { default: { track: async () => {} } },
    },
  );
  return {
    ...hooks,
    listeners,
    render: () => hooks.render(() => DeviceSelection({
      selectedDevices: { micDevice: selectedMic, systemDevice: null },
      onDeviceChange: () => {},
    })),
  };
}

test('the microphone test meters the actual default endpoint after a stale preference falls back', async () => {
  const view = createView({
    selectedMic: 'Microphone (1- USB Headset) (input)',
    monitoredDevices: [{
      requested_name: 'Microphone (1- USB Headset)',
      device_name: 'Microphone (2- USB Headset)',
      used_default: true,
    }],
  });
  try {
    view.render();
    await flush();
    const testButton = elements(view.render()).find(element => element.props?.children?.[1] === 'Test mic');
    assert.ok(testButton, 'The microphone test action must be available');
    await testButton.props.onClick();
    view.render();
    await flush();
    view.listeners.get('audio-levels')({ payload: {
      timestamp: 1,
      levels: [{
        device_name: 'Microphone (2- USB Headset)',
        device_type: 'input',
        rms_level: 0.25,
        peak_level: 0.5,
        is_active: true,
      }],
    } });
    const meter = elements(view.render()).find(element => element.type === 'AudioLevelMeter');
    assert.ok(meter, 'A successful fallback test must display the endpoint actually monitored');
    assert.equal(meter.props.deviceName, 'Microphone (2- USB Headset)');
    assert.equal(meter.props.rmsLevel, 0.25);
    assert.equal(meter.props.isActive, true);
  } finally {
    view.unmount();
    await flush();
  }
});
