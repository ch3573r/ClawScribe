import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, flush } from './hook-harness.mjs';

const jsx = (type, props) => ({ type, props });
const nodes = node => !node || typeof node !== 'object' ? [] : Array.isArray(node) ? node.flatMap(nodes)
  : [node, ...nodes(node.props?.children), ...nodes(node.props?.actions), ...nodes(node.props?.title), ...nodes(node.props?.description)];
const text = node => typeof node === 'string' || typeof node === 'number' ? String(node) : Array.isArray(node) ? node.map(text).join('')
  : node && typeof node === 'object' ? [node.props?.title, node.props?.description, node.props?.children].map(text).join('') : '';

const KEY = 'synthetic-placeholder-key-0001';

// ---- useCommitField and useSaveFeedback -------------------------------------------------

function commitHarness(options) {
  const hooks = createHookHarness();
  const mod = loadTsModule('src/components/settings/commit-field.ts', { react: hooks.react });
  const render = () => hooks.render(() => mod.useCommitField(options));
  return { hooks, mod, render };
}

test('a text field saves on blur and on Enter only, never while typing', async () => {
  const saved = [];
  const { render, hooks } = commitHarness({ value: 'https://old.example.com', onCommit: async value => { saved.push(value); } });
  let field = render();
  field.inputProps.onChange({ target: { value: 'https://new.example.com/v' } });
  field.inputProps.onChange({ target: { value: 'https://new.example.com/v1' } });
  await flush();
  assert.deepEqual(saved, [], 'typing alone never saves');
  field = render();
  field.inputProps.onBlur();
  await flush();
  assert.deepEqual(saved, ['https://new.example.com/v1']);

  field = render();
  field.inputProps.onChange({ target: { value: 'https://second.example.com' } });
  let prevented = false;
  field.inputProps.onKeyDown({ key: 'Enter', preventDefault() { prevented = true; } });
  field.inputProps.onBlur();
  await flush();
  assert.equal(prevented, true);
  assert.deepEqual(saved, ['https://new.example.com/v1', 'https://second.example.com'], 'Enter then blur saves once');

  field = render();
  field.inputProps.onKeyDown({ key: 'a', preventDefault() {} });
  field.inputProps.onBlur();
  await flush();
  assert.equal(saved.length, 2, 'an unchanged value is not saved again');
  hooks.unmount();
});

test('an invalid value is not saved and shows its error next to the field', async () => {
  const saved = [];
  const { render, hooks } = commitHarness({
    value: 'https://ok.example.com', validate: value => (/^https?:/.test(value) ? null : 'Enter a URL that starts with http:// or https://'),
    onCommit: async value => { saved.push(value); },
  });
  render().inputProps.onChange({ target: { value: 'not a url' } });
  await render().commit();
  const field = render();
  assert.deepEqual(saved, []);
  assert.match(field.error, /http:\/\//);
  assert.equal(field.draft, 'not a url', 'the draft is kept');
  field.inputProps.onChange({ target: { value: 'https://fixed.example.com' } });
  assert.equal(render().error, '', 'the error clears as soon as the user edits');
  hooks.unmount();
});

test('a failed save keeps the draft, shows a message and never shows the key', async () => {
  const { render, hooks, mod } = commitHarness({
    value: '', onCommit: async value => { throw new Error(`backend rejected ${value}`); },
  });
  render().inputProps.onChange({ target: { value: KEY } });
  await render().commit();
  const failed = render();
  assert.equal(failed.draft, KEY, 'what the user typed is kept');
  assert.equal(failed.status, 'idle', 'a failed save never reads as saved');
  assert.equal(failed.error, mod.GENERIC_SAVE_ERROR);
  assert.doesNotMatch(failed.error, new RegExp(KEY));
  hooks.unmount();
});

test('only SaveError messages are shown and secrets are scrubbed from backend strings', async () => {
  const { render, hooks, mod } = commitHarness({
    value: 'a', onCommit: async () => { throw new mod.SaveError('Could not save the API key'); },
  });
  render().inputProps.onChange({ target: { value: 'b' } });
  await render().commit();
  assert.equal(render().error, 'Could not save the API key');
  hooks.unmount();
  assert.equal(mod.safeMessage(`bad token ${KEY}`, 'fallback', [KEY]), 'fallback');
  assert.equal(mod.safeMessage('Destination is not allowed', 'fallback', [KEY]), 'Destination is not allowed');
  assert.equal(mod.safeMessage('x'.repeat(500), 'fallback'), 'fallback');
  assert.equal(mod.safeMessage({ secret: KEY }, 'fallback'), 'fallback');
});

test('a held value is kept without a saved confirmation and write-only fields clear after a save', async () => {
  const held = commitHarness({ value: '', onCommit: async () => 'held' });
  held.render().inputProps.onChange({ target: { value: 'kept' } });
  await held.render().commit();
  assert.equal(held.render().status, 'idle');
  assert.equal(held.render().draft, 'kept');
  held.hooks.unmount();

  const token = commitHarness({ value: '', clearAfterSave: true, onCommit: async () => {} });
  token.render().inputProps.onChange({ target: { value: KEY } });
  await token.render().commit();
  assert.equal(token.render().status, 'saved');
  assert.equal(token.render().draft, '');
  token.hooks.unmount();
});

test('selects and switches report saving, saved and failed states per control', async () => {
  const hooks = createHookHarness();
  const mod = loadTsModule('src/components/settings/commit-field.ts', { react: hooks.react });
  const render = () => hooks.render(() => mod.useSaveFeedback());
  const ok = await render().run('provider', async () => {});
  assert.equal(ok, true);
  assert.equal(render().states.provider.status, 'saved');
  const failed = await render().run('model', async () => { throw new Error(`raw ${KEY}`); });
  assert.equal(failed, false);
  assert.equal(render().states.model.status, 'error');
  assert.doesNotMatch(render().states.model.message, new RegExp(KEY));
  hooks.unmount();
});

// ---- What this sends ---------------------------------------------------------------------

test('the data panel describes audio use for summaries without asserting a transcription policy', () => {
  const { describeProviderData } = loadTsModule('src/components/settings/provider-data.ts');
  const claudeCode = describeProviderData('claude-code');
  assert.equal(claudeCode.local, false);
  assert.deepEqual(Array.from(claudeCode.items, item => item.title), ['Transcript text', 'Template and language', 'Audio']);
  for (const provider of ['claude-code', 'claude', 'groq', 'openrouter', 'custom-openai', 'codex', 'builtin-ai', 'ollama']) {
    const audio = describeProviderData(provider).items.find(item => item.title === 'Audio');
    assert.equal(audio?.detail, 'Summary requests do not include audio', provider);
  }
  for (const [provider, endpoint] of [['builtin-ai', null], ['ollama', null], ['ollama', 'http://localhost:11434'], ['ollama', 'http://127.0.0.1:11434']]) {
    const data = describeProviderData(provider, endpoint);
    assert.equal(data.local, true, `${provider} ${endpoint}`);
    assert.equal(data.items[0].title, 'Local summary processing');
  }
  const remote = describeProviderData('ollama', 'http://ollama.example.com:11434');
  assert.equal(remote.local, false, 'an Ollama server on another host receives the transcript');
});

test('See everything in Privacy opens the Privacy tab', () => {
  const events = [];
  globalThis.window = { dispatchEvent: event => events.push(event) };
  const { ProviderDataAside } = loadTsModule('src/components/settings/summary-provider-parts.tsx', {
    '@/components/ui/button': { Button: 'button' }, '@/components/ui/input': { Input: 'input' }, '@/components/ui/page-section': { PageSection: 'section' },
  });
  const tree = ProviderDataAside({ provider: 'claude-code' });
  assert.match(text(tree), /Transcript text/);
  assert.match(text(tree), /With Claude Code as the provider/);
  const link = nodes(tree).find(node => node.type === 'button' && /Privacy/.test(text(node)));
  link.props.onClick();
  assert.equal(events.length, 1);
  assert.equal(events[0].type, 'open-settings-tab');
  assert.equal(events[0].detail, 'privacy');
  delete globalThis.window;
});

// ---- Model settings: immediate-save mode versus the explicit Save button ----------------

function modalHarness({ provider = 'claude', stored = false, onSave, catalogResponse, autosave = true, model = 'claude-sonnet-5' } = {}) {
  const hooks = createHookHarness();
  const calls = [];
  // The core never returns a stored key: the settings only know whether one is stored.
  const config = { provider, model, whisperModel: 'large-v3', apiKey: null, ollamaEndpoint: null };
  const context = {
    modelConfig: config,
    setModelConfig(next) { Object.assign(context, { modelConfig: typeof next === 'function' ? next(context.modelConfig) : next }); },
    providerKeyStatus: { claude: { stored, hint: stored ? '0001' : null }, groq: { stored: false, hint: null }, openai: { stored: false, hint: null }, openrouter: { stored: false, hint: null } },
    refreshProviderKeyStatus: async key => { context.providerKeyStatus = { ...context.providerKeyStatus, [key]: { stored: true, hint: null } }; },
  };
  const stub = name => ({ [name]: name });
  const mocks = {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx, Fragment: 'fragment' },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push([command, args]);
      if (command === 'get_anthropic_models') return catalogResponse ? catalogResponse() : [{ id: 'claude-sonnet-5' }];
      if (command === 'get_groq_models') return [];
      if (command === 'codex_get_config') return {};
      return null;
    } },
    '@tauri-apps/plugin-os': { platform: () => 'windows' },
    '@/lib/openExternal': { openExternal() {} },
    '@/components/UnencryptedHttpOptIn': { UnencryptedHttpOptIn: 'http-opt-in' },
    './Sidebar/SidebarProvider': { useSidebar: () => ({ serverAddress: null }) },
    '@/contexts/OllamaDownloadContext': { useOllamaDownload: () => ({ isDownloading: () => false, getProgress: () => undefined, downloadingModels: new Set() }) },
    '@/components/BuiltInModelManager': { BuiltInModelManager: 'built-in-models' },
    '@/contexts/ConfigContext': { useConfig: () => context },
    './ClaudeCodeProviderCard': {
      ClaudeCodeProviderCard: 'claude-code-card', CLAUDE_CODE_MODELS: ['sonnet', 'opus', 'haiku'],
      useClaudeCodeSettings: () => ({ status: null, programPath: '', setProgramPath() {}, busy: false, check: async () => {}, save: async () => {} }),
    },
    '@/components/ui/button': { Button: 'button' }, '@/components/ui/input': { Input: 'input' }, '@/components/ui/label': { Label: 'label' },
    '@/components/ui/select': { Select: 'select', SelectContent: 'select-content', SelectGroup: 'select-group', SelectItem: 'select-item', SelectLabel: 'select-label', SelectTrigger: 'select-trigger', SelectValue: 'select-value' },
    '@/components/ui/alert': { Alert: 'alert', AlertDescription: 'alert-description' }, '@/components/ui/scroll-area': { ScrollArea: 'scroll-area' },
    '@/components/ui/switch': { Switch: 'switch' }, '@/components/ui/progress': { Progress: 'progress' },
    '@/components/ui/popover': { Popover: 'popover', PopoverContent: 'popover-content', PopoverTrigger: 'popover-trigger' },
    '@/components/ui/command': { Command: 'command', CommandEmpty: 'command-empty', CommandGroup: 'command-group', CommandInput: 'command-input', CommandItem: 'command-item', CommandList: 'command-list' },
    '@/components/settings/SecretKeyControl': { SecretKeyControl: 'secret-key-control', SecretKeyRow: 'secret-key-row' },
    '@/services/configService': { configService: { clearApiKey: async () => {}, clearCustomOpenAIApiKey: async () => {}, getCustomOpenAIConfig: async () => null } },
    '@/components/settings/settings-parts': { NoteRow: 'note-row', ROW_SELECT: 'w-60' },
    '@/components/settings/summary-provider-parts': {
      CommitInputRow: 'commit-input-row', CopyCommandRow: 'copy-command-row', SaveMark: 'save-mark',
      withError: (description, error) => (error ? [description, error] : description),
    },
    sonner: { toast: { error() {}, success() {}, info() {} } },
    'lucide-react': new Proxy({}, { get: (_target, name) => (name === '__esModule' ? false : String(name)) }),
  };
  const { ModelSettingsModal } = loadTsModule('src/components/ModelSettingsModal.tsx', mocks);
  const saves = [];
  const props = {
    modelConfig: config, setModelConfig: context.setModelConfig, skipInitialFetch: true, autosave,
    onSave: onSave ?? (async next => { saves.push(next); }),
  };
  const render = () => hooks.render(() => ModelSettingsModal(props));
  return { hooks, calls, saves, context, render };
}

test('choosing another provider only previews it; the saved provider stays in use until the user switches', async () => {
  const app = modalHarness({ provider: 'claude', stored: true });
  app.render(); await flush(); app.render();
  const select = () => nodes(app.render()).find(node => node.type === 'select' && ['claude', 'codex', 'groq'].includes(node.props.value));
  const button = label => nodes(app.render()).find(node => node.type === 'button' && text(node) === label);
  select().props.onValueChange('codex');
  await flush();
  assert.equal(app.saves.length, 0, 'looking at a provider saves nothing');
  assert.equal(select().props.value, 'codex', 'the page shows the provider being looked at');
  assert.equal(app.context.modelConfig.provider, 'claude', 'summaries keep using the saved provider: the shared configuration is untouched');
  assert.match(text(app.render()), /Not in use yet/);
  assert.match(text(app.render()), /Claude stays your summary provider until you switch/);
  await flush();
  assert.equal(app.calls.filter(([command]) => command === 'codex_get_config').length, 1, 'opening the Codex preview checks its configuration once');

  button('Keep Claude').props.onClick();
  await flush();
  assert.equal(app.saves.length, 0);
  assert.equal(select().props.value, 'claude', 'going back shows the saved setup again');
  assert.equal(app.context.modelConfig.provider, 'claude');
  assert.doesNotMatch(text(app.render()), /Not in use yet/);

  select().props.onValueChange('codex');
  await flush();
  button('Use Codex').props.onClick();
  await flush();
  assert.equal(app.saves.length, 1);
  assert.equal(app.saves[0].provider, 'codex');
  assert.equal(app.context.modelConfig.provider, 'codex', 'the shared configuration changes only after the switch is saved');
  assert.doesNotMatch(text(app.render()), /Not in use yet/);
  assert.equal(nodes(app.render()).some(node => node.type === 'button' && text(node) === 'Save'), false, 'no Save button on the settings page');
  app.hooks.unmount();
});

test('a provider that still needs a key cannot be switched to, and a failed switch shows no key', async () => {
  const app = modalHarness({ provider: 'claude', stored: true });
  app.render(); await flush(); app.render();
  nodes(app.render()).find(node => node.type === 'select' && node.props.value === 'claude').props.onValueChange('groq');
  await flush();
  const use = nodes(app.render()).find(node => node.type === 'button' && text(node) === 'Use Groq');
  assert.equal(use.props.disabled, true);
  assert.match(text(app.render()), /Enter an API key first/);
  assert.equal(app.saves.length, 0);

  const failing = modalHarness({ provider: 'claude', stored: true, onSave: async () => { throw new Error(`database said ${KEY}`); } });
  failing.render(); await flush(); failing.render();
  nodes(failing.render()).find(node => node.type === 'select' && node.props.value === 'claude').props.onValueChange('codex');
  await flush();
  nodes(failing.render()).find(node => node.type === 'button' && text(node) === 'Use Codex').props.onClick();
  await flush();
  const shown = text(failing.render());
  assert.match(shown, /Could not save the provider/);
  assert.match(shown, /Not in use yet/, 'a failed switch leaves the saved provider in use');
  assert.equal(failing.context.modelConfig.provider, 'claude');
  assert.doesNotMatch(shown, new RegExp(KEY));
  app.hooks.unmount(); failing.hooks.unmount();
});

test('a key entered for a provider that is only being looked at is held, not saved', async () => {
  const app = modalHarness({ provider: 'claude', stored: true });
  app.render(); await flush(); app.render();
  nodes(app.render()).find(node => node.type === 'select' && node.props.value === 'claude').props.onValueChange('groq');
  await flush();
  const other = 'synthetic-placeholder-key-0002';
  const field = () => nodes(app.render()).find(node => node.type === 'secret-key-row' && node.props.id === 'summary-api-key');
  assert.equal(field().props.draft, '', 'nothing is prefilled: no stored key reaches the field');
  assert.equal(field().props.onCommit, undefined, 'a provider that is only being looked at has no commit-on-blur field');
  field().props.onDraftChange(other);
  await flush();
  assert.equal(app.saves.length, 0, 'entering a key does not switch the provider');
  assert.equal(app.context.modelConfig.provider, 'claude');
  assert.equal(app.context.modelConfig.apiKey, null, 'the saved provider keeps its own key; a typed key is not put in the shared configuration');
  assert.match(text(app.render()), /Not in use yet/);
  assert.equal(field().props.draft, other, 'the typed key is held with the preview');
  // Using the provider saves the typed key, once, and then forgets it.
  nodes(app.render()).find(node => node.type === 'button' && text(node) === 'Use Groq').props.onClick();
  await flush();
  assert.equal(app.saves.length, 1);
  assert.equal(app.saves[0].provider, 'groq');
  assert.equal(app.saves[0].apiKey, other);
  assert.equal(app.context.modelConfig.apiKey ?? null, null, 'after the switch no key stays in the shared configuration');
  app.hooks.unmount();
});

test('an API key is saved by committing the field and a half-typed key is never used to check the provider', async () => {
  const app = modalHarness({ provider: 'claude', stored: false });
  app.render(); await flush(); app.render();
  assert.equal(app.calls.some(([command]) => command === 'get_anthropic_models'), false, 'no model request without a saved key');
  const field = () => nodes(app.render()).find(node => node.type === 'secret-key-row' && node.props.id === 'summary-api-key');
  assert.equal(field().props.status.stored, false);
  await field().props.onCommit(KEY);
  assert.equal(app.saves.length, 1);
  assert.equal(app.saves[0].apiKey, KEY);
  assert.equal(app.context.modelConfig.apiKey ?? null, null, 'a saved key is not kept in the configuration');
  app.render(); await flush(); app.render(); await flush();
  assert.equal(app.calls.filter(([command]) => command === 'get_anthropic_models').length, 1, 'the committed key loads the model list');
  app.hooks.unmount();
});

test('switching providers in the explicit-save dialog discards the previous provider’s typed key before model discovery', async () => {
  const app = modalHarness({ provider: 'claude', autosave: false });
  app.render(); await flush();
  const keyField = () => nodes(app.render()).find(node => node.props.id === 'legacy-api-key');
  keyField().props.onDraftChange(KEY);
  app.render(); await flush();
  app.context.setModelConfig(prev => ({ ...prev, provider: 'groq', model: 'llama-3.3-70b-versatile' }));
  const nextField = keyField();
  await flush();
  assert.equal(nextField.props.draft, '');
  assert.equal(app.calls.some(([command]) => command === 'get_groq_models'), false);
  app.hooks.unmount();
});

test('a failed key save raises a message that does not contain the key', async () => {
  const app = modalHarness({ provider: 'claude', stored: false, onSave: async () => { throw new Error(`rejected ${KEY}`); } });
  app.render(); await flush(); app.render();
  const field = nodes(app.render()).find(node => node.type === 'secret-key-row' && node.props.id === 'summary-api-key');
  await assert.rejects(() => field.props.onCommit(KEY), error => {
    assert.equal(error.name, 'SaveError');
    assert.doesNotMatch(error.message, new RegExp(KEY));
    return true;
  });
  assert.equal(app.context.modelConfig.apiKey, null, 'the saved configuration is unchanged after a failed save');
  app.hooks.unmount();
});

test('the same component keeps its explicit Save button and draft behaviour for other callers', async () => {
  const app = modalHarness({ provider: 'claude', stored: true, autosave: false });
  app.render(); await flush();
  const tree = app.render();
  const save = nodes(tree).find(node => node.type === 'button' && text(node) === 'Save');
  assert.ok(save, 'the Save button is present');
  assert.equal(nodes(tree).some(node => node.type === 'commit-input-row' || node.type === 'secret-key-row'), false);
  assert.equal(app.saves.length, 0, 'nothing saves until Save is pressed');
  assert.equal(nodes(tree).some(node => node.type === 'select' && node.props.value === 'claude'), true, 'the provider select is the original one');
  app.hooks.unmount();
});

test('hosted catalog failures distinguish a rejected key, denied model listing, and a provider outage', async () => {
  for (const [failure, message] of [
    ['invalid_api_key', /provider rejected this API key/i],
    ['catalog_forbidden', /key cannot list models/i],
    ['catalog_unavailable', /could not verify this key/i],
  ]) {
    for (const autosave of [true, false]) {
      const app = modalHarness({
        stored: true, autosave,
        catalogResponse: () => Promise.reject(failure),
      });
      app.render(); await flush();
      const rendered = text(app.render());
      assert.match(rendered, message);
      assert.doesNotMatch(rendered, /Model list verified with this key/);
      app.hooks.unmount();
    }
  }
});

test('a fresh summary setup shows the local draft without claiming a selected model or saving a provider', async () => {
  const { UNCONFIGURED_SUMMARY_MODEL } = loadTsModule('src/constants/modelDefaults');
  const app = modalHarness(UNCONFIGURED_SUMMARY_MODEL);
  app.render();
  await flush();
  assert.match(text(app.render()), /No summary model selected/);
  assert.equal(app.context.modelConfig.provider, 'builtin-ai');
  assert.equal(app.context.modelConfig.model, '');
  assert.equal(app.saves.length, 0);
  app.hooks.unmount();
});
