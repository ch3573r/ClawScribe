import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const names = value => Object.fromEntries(value.split(' ').map(name => [name, name]));
const jsx = (type, props) => ({ type, props });
const nodes = root => Array.isArray(root) ? root.flatMap(nodes) : root && typeof root === 'object'
  ? [root, ...nodes(root.props?.children)] : [];
const text = root => typeof root === 'string' ? root : Array.isArray(root) ? root.map(text).join('') : text(root?.props?.children ?? '');

for (const provider of ['cloud-whisper', 'mai-transcribe']) {
  test(`${provider} tests never submit an automatically loaded key as a new key`, async () => {
    const hooks = createHookHarness(), calls = [];
    const { TranscriptSettings } = loadTsModule('src/components/TranscriptSettings.tsx', {
      react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
      'lucide-react': names('Eye EyeOff FlaskConical Loader2 Lock Unlock'),
      './ui/select': names('Select SelectContent SelectGroup SelectItem SelectLabel SelectTrigger SelectValue'),
      './ui/input': names('Input'), './ui/button': names('Button'), './ui/label': names('Label'),
      './WhisperModelManager': names('ModelManager'), './ParakeetModelManager': names('ParakeetModelManager'),
      './NemotronModelManager': names('NemotronModelManager'), './WhisperAccelerationStatus': names('WhisperAccelerationStatus'),
      './UnencryptedHttpOptIn': names('UnencryptedHttpOptIn'),
      '@/hooks/useCloudTranscription': { useCloudTranscription: () => true },
      '@tauri-apps/plugin-dialog': { open: async () => 'test.wav' },
      '@tauri-apps/api/core': { invoke: async (command, args) => {
        calls.push({ command, args });
        return { segmentCount: 1, wordTimestampCount: 0, previewText: 'Synthetic text' };
      } },
      sonner: { toast: { success() {}, error(message) { assert.fail(message); } } },
    });
    let config = { provider, model: 'test-model', apiKey: 'test-token',
      baseUrl: 'http://model.local/v1', endpoint: 'http://speech.local', allowUnencrypted: true };
    const render = () => hooks.render(() => TranscriptSettings({
      transcriptModelConfig: config, setTranscriptModelConfig: value => { config = value; },
    }));
    const button = label => nodes(render()).find(node => node.type === 'Button' && text(node).trim() === label);
    render();
    assert.equal(nodes(render()).find(node => node.type === 'UnencryptedHttpOptIn').props.checked, true);
    const endpoint = provider === 'cloud-whisper' ? config.baseUrl : config.endpoint;
    nodes(render()).find(node => node.type === 'Input' && node.props.value === endpoint)
      .props.onChange({ target: { value: 'https://other.example.com/v1' } });
    await button('Test').props.onClick();
    assert.equal(calls.at(-1).args.apiKey, null, 'backend must bind the saved key to its endpoint');
    assert.equal(calls.at(-1).args.allowUnencrypted, true);
    nodes(render()).find(node => node.type === 'Input' && node.props.placeholder === 'Enter your API key')
      .props.onChange({ target: { value: 'test-token' } });
    await button('Test').props.onClick();
    assert.equal(calls.at(-1).args.apiKey, 'test-token', 'explicitly entered key can test an edited endpoint');
    await button('Save').props.onClick();
    assert.equal(calls.at(-1).command, 'api_save_transcript_config');
    assert.equal(calls.at(-1).args.allowUnencrypted, true);
    await button('Test').props.onClick();
    assert.equal(calls.at(-1).args.apiKey, null, 'after save the key is bound again');
    hooks.unmount();
  });
}
