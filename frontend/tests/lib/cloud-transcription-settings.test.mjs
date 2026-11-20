import assert from 'node:assert/strict';
import test from 'node:test';
import { createHookHarness, flush } from './hook-harness.mjs';
import { loadTsModule } from './load-ts-module.mjs';

const names = value => Object.fromEntries(value.split(' ').map(name => [name, name]));
const jsx = (type, props) => ({ type, props });
// Shared layout components take some content as props, so look there as well as at children.
const slots = props => [props?.title, props?.children, props?.actions, props?.description];
const nodes = root => Array.isArray(root) ? root.flatMap(nodes) : root && typeof root === 'object'
  ? [root, ...slots(root.props).flatMap(nodes)] : [];
const text = root => typeof root === 'string' ? root : Array.isArray(root) ? root.map(text).join('')
  : root && typeof root === 'object' ? slots(root.props).map(text).join('') : '';

for (const provider of ['cloud-whisper', 'mai-transcribe']) {
  test(`${provider} tests never submit an automatically loaded key as a new key`, async () => {
    const hooks = createHookHarness(), calls = [];
    const { TranscriptSettings } = loadTsModule('src/components/TranscriptSettings.tsx', {
      react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
      'next/navigation': { useRouter: () => ({ push() {} }) },
      'lucide-react': names('FlaskConical Loader2'),
      './ui/select': names('Select SelectContent SelectGroup SelectItem SelectLabel SelectTrigger SelectValue'),
      './ui/input': names('Input'), './ui/button': names('Button'), './ui/label': names('Label'),
      './WhisperModelManager': names('ModelManager'), './ParakeetModelManager': names('ParakeetModelManager'),
      './NemotronModelManager': names('NemotronModelManager'), './WhisperAccelerationStatus': names('WhisperAccelerationStatus'),
      './UnencryptedHttpOptIn': names('UnencryptedHttpOptIn'), './TranscriptionPcTest': names('TranscriptionPcTest'),
      '@/components/ui/settings-layout': names('SettingsCard SettingsFooter SettingsRow'),
      '@/components/settings/SecretKeyControl': names('SecretKeyRow SecretKeyControl'),
      '@/services/configService': { configService: { getTranscriptApiKeyStatus: async () => ({ stored: true, hint: null }), clearTranscriptApiKey: async () => {} } },
      '@/hooks/useCloudTranscription': { useCloudTranscription: () => ({ enabled: true, loaded: true }) },
      '@tauri-apps/api/core': { invoke: async (command, args) => {
        if (command === 'pick_transcription_test_audio') return 'test.wav';
        calls.push({ command, args });
        return { segmentCount: 1, wordTimestampCount: 0, previewText: 'Synthetic text' };
      } },
      sonner: { toast: { success() {}, error(message) { assert.fail(message); } } },
    });
    let config = { provider, model: 'test-model',
      baseUrl: 'http://model.local/v1', endpoint: 'http://speech.local', allowUnencrypted: true };
    const render = () => hooks.render(() => TranscriptSettings({
      transcriptModelConfig: config, setTranscriptModelConfig: value => { config = value; },
    }));
    const button = label => nodes(render()).find(node => node.type === 'Button' && text(node).trim() === label);
    const keyRow = () => nodes(render()).find(node => node.type === 'SecretKeyRow');
    render();
    await new Promise(resolve => setTimeout(resolve, 0));
    assert.equal(nodes(render()).find(node => node.type === 'UnencryptedHttpOptIn').props.checked, true);
    // The stored key is never loaded into the field: it only reports that one is stored.
    assert.equal(keyRow().props.status.stored, true);
    assert.equal(keyRow().props.draft, '');
    const endpoint = provider === 'cloud-whisper' ? config.baseUrl : config.endpoint;
    nodes(render()).find(node => node.type === 'Input' && node.props.value === endpoint)
      .props.onChange({ target: { value: 'https://other.example.com/v1' } });
    await button('Test').props.onClick();
    assert.equal(calls.at(-1).command, 'api_test_transcript_provider');
    assert.equal(calls.at(-1).args.apiKey, null, 'no key is sent unless one was typed; the core binds the stored key to its endpoint');
    assert.equal(calls.at(-1).args.audioPath, 'test.wav', 'the file is the one the core chose in its own dialog');
    assert.equal(calls.at(-1).args.allowUnencrypted, true);
    keyRow().props.onDraftChange('test-token');
    await button('Test').props.onClick();
    assert.equal(calls.at(-1).args.apiKey, 'test-token', 'an explicitly entered key can test an edited endpoint');
    await button('Save').props.onClick();
    assert.equal(calls.at(-1).command, 'api_save_transcript_config');
    assert.equal(calls.at(-1).args.apiKey, 'test-token');
    assert.equal(calls.at(-1).args.allowUnencrypted, true);
    assert.equal(config.apiKey ?? null, null, 'a saved key is not kept in the shared configuration');
    assert.equal(keyRow().props.draft, '', 'the typed key is forgotten once it is saved');
    await button('Test').props.onClick();
    assert.equal(calls.at(-1).args.apiKey, null, 'after save the stored key is used by the core again');
    // Saving without touching the key field sends no key, so the stored one is kept.
    await button('Save').props.onClick();
    assert.equal(calls.at(-1).command, 'api_save_transcript_config');
    assert.equal(calls.at(-1).args.apiKey, null);
    hooks.unmount();
  });
}

for (const enabled of [true, false]) {
  test(`saved MAI survives preference loading (${enabled}) and explains live transcription`, async t => {
    const hooks = createHookHarness(), saves = [], routes = [];
    const previousWindow = globalThis.window, previousEvent = globalThis.CustomEvent;
    const events = [];
    globalThis.window = { dispatchEvent: event => events.push(event) };
    globalThis.CustomEvent = class { constructor(type, options) { this.type = type; this.detail = options.detail; } };
    t.after(() => { globalThis.window = previousWindow; globalThis.CustomEvent = previousEvent; });
    let preferenceChanged;
    const { useCloudTranscription } = loadTsModule('src/hooks/useCloudTranscription.ts', {
      react: hooks.react,
      '@/lib/behaviorSettings': { behaviorSettingsLoaded: () => false, loadBehaviorSettings: async () => ({}) },
      '@/lib/cloudTranscription': {
        getCloudTranscription: () => enabled,
        subscribeCloudTranscription: callback => { preferenceChanged = callback; return () => {}; },
      },
    });
    const { TranscriptSettings } = loadTsModule('src/components/TranscriptSettings.tsx', {
      react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
      'next/navigation': { useRouter: () => ({ push: route => routes.push(route) }) },
      'lucide-react': names('FlaskConical Loader2'),
      './ui/select': names('Select SelectContent SelectGroup SelectItem SelectLabel SelectTrigger SelectValue'),
      './ui/input': names('Input'), './ui/button': names('Button'), './ui/label': names('Label'),
      './WhisperModelManager': names('ModelManager'), './ParakeetModelManager': names('ParakeetModelManager'),
      './NemotronModelManager': names('NemotronModelManager'), './WhisperAccelerationStatus': names('WhisperAccelerationStatus'),
      './UnencryptedHttpOptIn': names('UnencryptedHttpOptIn'), './TranscriptionPcTest': names('TranscriptionPcTest'),
      '@/components/ui/settings-layout': names('SettingsCard SettingsFooter SettingsRow'),
      '@/components/settings/SecretKeyControl': names('SecretKeyRow SecretKeyControl'),
      '@/services/configService': { configService: { getTranscriptApiKeyStatus: async () => ({ stored: true, hint: null }), clearTranscriptApiKey: async () => {} } },
      '@/hooks/useCloudTranscription': { useCloudTranscription },
      '@tauri-apps/plugin-dialog': { open: async () => null },
      '@tauri-apps/api/core': { invoke: async () => null },
      sonner: { toast: { success() {}, error() {} } },
    });
    const render = () => hooks.render(() => TranscriptSettings({
      transcriptModelConfig: { provider: 'mai-transcribe', model: 'mai-transcribe-1.5' },
      setTranscriptModelConfig: value => saves.push(value),
    }));
    const first = render();
    assert.equal(nodes(first).find(node => node.type === 'Select').props.value, 'mai-transcribe');
    assert.doesNotMatch(text(first), /Cloud transcription is off/, 'nothing is claimed until the core has answered');
    await flush();
    for (let i = 0; i < 3; i++) {
      const tree = render();
      assert.equal(nodes(tree).find(node => node.type === 'Select').props.value, 'mai-transcribe');
      assert.match(text(tree), /Live recordings always transcribe on this device with Parakeet \(parakeet-tdt-0.6b-v3-int8\)/);
      assert.equal(text(tree).includes('Cloud transcription is off'), !enabled);
      assert.equal(nodes(tree).some(node => node.type === 'SelectItem' && node.props.value === 'mai-transcribe'), true);
    }
    if (!enabled) {
      nodes(render()).find(node => node.type === 'Button' && text(node) === 'Open options').props.onClick();
      assert.deepEqual(routes, ['/settings?tab=transcription']);
      assert.equal(events[0].detail, 'transcription');
    }
    preferenceChanged(!enabled);
    assert.equal(nodes(render()).find(node => node.type === 'Select').props.value, 'mai-transcribe');
    assert.equal(saves.length, 0);
    hooks.unmount();
  });
}

test('"Open options" scrolls to the Options card on the Transcription page instead of navigating', t => {
  const hooks = createHookHarness(), routes = [], moves = [];
  const previousDocument = globalThis.document;
  globalThis.document = {
    getElementById: id => id === 'transcription-options'
      ? { scrollIntoView: arg => moves.push(['scroll', arg.block]), focus: arg => moves.push(['focus', arg.preventScroll]) } : null,
  };
  t.after(() => { globalThis.document = previousDocument; hooks.unmount(); });
  const { TranscriptSettings } = loadTsModule('src/components/TranscriptSettings.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    'next/navigation': { useRouter: () => ({ push: route => routes.push(route) }) },
    'lucide-react': names('FlaskConical Loader2'),
    './ui/select': names('Select SelectContent SelectGroup SelectItem SelectLabel SelectTrigger SelectValue'),
    './ui/input': names('Input'), './ui/button': names('Button'),
    './WhisperModelManager': names('ModelManager'), './ParakeetModelManager': names('ParakeetModelManager'),
    './NemotronModelManager': names('NemotronModelManager'), './WhisperAccelerationStatus': names('WhisperAccelerationStatus'),
    './UnencryptedHttpOptIn': names('UnencryptedHttpOptIn'), './TranscriptionPcTest': names('TranscriptionPcTest'),
    '@/components/ui/settings-layout': names('SettingsCard SettingsFooter SettingsRow'),
      '@/components/settings/SecretKeyControl': names('SecretKeyRow SecretKeyControl'),
      '@/services/configService': { configService: { getTranscriptApiKeyStatus: async () => ({ stored: true, hint: null }), clearTranscriptApiKey: async () => {} } },
    '@/hooks/useCloudTranscription': { useCloudTranscription: () => ({ enabled: false, loaded: true }) },
    '@tauri-apps/plugin-dialog': { open: async () => null },
    '@tauri-apps/api/core': { invoke: async () => null },
    sonner: { toast: { success() {}, error() {} } },
  });
  const tree = hooks.render(() => TranscriptSettings({
    transcriptModelConfig: { provider: 'cloud-whisper', model: 'whisper-1' }, setTranscriptModelConfig() {},
  }));
  nodes(tree).find(node => node.type === 'Button' && text(node) === 'Open options').props.onClick();
  assert.deepEqual(moves, [['scroll', 'start'], ['focus', true]]);
  assert.deepEqual(routes, []);
});
