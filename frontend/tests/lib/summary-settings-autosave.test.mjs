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

test('the data panel follows the selected provider and never claims audio leaves the PC', () => {
  const { describeProviderData } = loadTsModule('src/components/settings/provider-data.ts');
  const claudeCode = describeProviderData('claude-code');
  assert.equal(claudeCode.local, false);
  assert.deepEqual(Array.from(claudeCode.items, item => item.title), ['Transcript text', 'Template and language', 'Audio']);
  for (const provider of ['claude-code', 'claude', 'groq', 'openrouter', 'custom-openai', 'codex', 'openclaw', 'builtin-ai', 'ollama']) {
    const audio = describeProviderData(provider).items.find(item => item.title === 'Audio');
    assert.equal(audio?.detail, 'Never leaves this PC', provider);
  }
  for (const [provider, endpoint] of [['builtin-ai', null], ['ollama', null], ['ollama', 'http://localhost:11434'], ['ollama', 'http://127.0.0.1:11434']]) {
    const data = describeProviderData(provider, endpoint);
    assert.equal(data.local, true, `${provider} ${endpoint}`);
    assert.equal(data.items[0].title, 'Nothing leaves this PC');
  }
  const remote = describeProviderData('ollama', 'http://ollama.example.com:11434');
  assert.equal(remote.local, false, 'an Ollama server on another host receives the transcript');
  assert.equal(describeProviderData('openclaw').items.some(item => item.title === 'Completed meetings'), true);
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

function modalHarness({ provider = 'claude', apiKey = null, onSave, autosave = true, model = 'claude-sonnet-5' } = {}) {
  const hooks = createHookHarness();
  const calls = [];
  const config = { provider, model, whisperModel: 'large-v3', apiKey, ollamaEndpoint: null };
  const context = {
    modelConfig: config,
    setModelConfig(next) { Object.assign(context, { modelConfig: typeof next === 'function' ? next(context.modelConfig) : next }); },
    providerApiKeys: { claude: apiKey, groq: null, openai: null, openrouter: null },
    updateProviderApiKey() {},
  };
  const stub = name => ({ [name]: name });
  const mocks = {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx, Fragment: 'fragment' },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push([command, args]);
      if (command === 'get_anthropic_models') return [{ id: 'claude-sonnet-5' }];
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
  const app = modalHarness({ provider: 'claude', apiKey: KEY });
  app.render(); await flush(); app.render();
  const select = () => nodes(app.render()).find(node => node.type === 'select' && node.props.value === app.context.modelConfig.provider);
  const button = label => nodes(app.render()).find(node => node.type === 'button' && text(node) === label);
  select().props.onValueChange('codex');
  await flush();
  assert.equal(app.saves.length, 0, 'looking at a provider saves nothing');
  assert.equal(app.context.modelConfig.provider, 'codex');
  assert.match(text(app.render()), /Not in use yet/);
  assert.match(text(app.render()), /Claude stays your summary provider until you switch/);

  button('Keep Claude').props.onClick();
  await flush();
  assert.equal(app.saves.length, 0);
  assert.equal(app.context.modelConfig.provider, 'claude', 'going back restores the saved setup');
  assert.doesNotMatch(text(app.render()), /Not in use yet/);

  select().props.onValueChange('codex');
  await flush();
  button('Use Codex').props.onClick();
  await flush();
  assert.equal(app.saves.length, 1);
  assert.equal(app.saves[0].provider, 'codex');
  assert.doesNotMatch(text(app.render()), /Not in use yet/);
  assert.equal(nodes(app.render()).some(node => node.type === 'button' && text(node) === 'Save'), false, 'no Save button on the settings page');
  app.hooks.unmount();
});

test('a provider that still needs a key cannot be switched to, and a failed switch shows no key', async () => {
  const app = modalHarness({ provider: 'claude', apiKey: KEY });
  app.render(); await flush(); app.render();
  nodes(app.render()).find(node => node.type === 'select' && node.props.value === 'claude').props.onValueChange('groq');
  await flush();
  const use = nodes(app.render()).find(node => node.type === 'button' && text(node) === 'Use Groq');
  assert.equal(use.props.disabled, true);
  assert.match(text(app.render()), /Enter an API key first/);
  assert.equal(app.saves.length, 0);

  const failing = modalHarness({ provider: 'claude', apiKey: KEY, onSave: async () => { throw new Error(`database said ${KEY}`); } });
  failing.render(); await flush(); failing.render();
  nodes(failing.render()).find(node => node.type === 'select' && node.props.value === 'claude').props.onValueChange('codex');
  await flush();
  nodes(failing.render()).find(node => node.type === 'button' && text(node) === 'Use Codex').props.onClick();
  await flush();
  const shown = text(failing.render());
  assert.match(shown, /Could not save the provider/);
  assert.match(shown, /Not in use yet/, 'a failed switch leaves the saved provider in use');
  assert.doesNotMatch(shown, new RegExp(KEY));
  app.hooks.unmount(); failing.hooks.unmount();
});

test('an API key is saved by committing the field and a half-typed key is never used to check the provider', async () => {
  const app = modalHarness({ provider: 'claude', apiKey: null });
  app.render(); await flush(); app.render();
  assert.equal(app.calls.some(([command]) => command === 'get_anthropic_models'), false, 'no model request without a saved key');
  const field = () => nodes(app.render()).find(node => node.type === 'commit-input-row' && node.props.id === 'summary-api-key');
  assert.match(field().props.validate(''), /required/);
  assert.equal(field().props.validate(KEY), null);
  await field().props.onCommit(KEY);
  assert.equal(app.saves.length, 1);
  assert.equal(app.saves[0].apiKey, KEY);
  app.render(); await flush();
  assert.equal(app.calls.filter(([command]) => command === 'get_anthropic_models').length, 1, 'the committed key loads the model list');
  app.hooks.unmount();
});

test('a failed key save raises a message that does not contain the key', async () => {
  const app = modalHarness({ provider: 'claude', apiKey: null, onSave: async () => { throw new Error(`rejected ${KEY}`); } });
  app.render(); await flush(); app.render();
  const field = nodes(app.render()).find(node => node.type === 'commit-input-row' && node.props.id === 'summary-api-key');
  await assert.rejects(() => field.props.onCommit(KEY), error => {
    assert.equal(error.name, 'SaveError');
    assert.doesNotMatch(error.message, new RegExp(KEY));
    return true;
  });
  assert.equal(app.context.modelConfig.apiKey, null, 'the saved configuration is unchanged after a failed save');
  app.hooks.unmount();
});

test('the same component keeps its explicit Save button and draft behaviour for other callers', async () => {
  const app = modalHarness({ provider: 'claude', apiKey: KEY, autosave: false });
  app.render(); await flush();
  const tree = app.render();
  const save = nodes(tree).find(node => node.type === 'button' && text(node) === 'Save');
  assert.ok(save, 'the Save button is present');
  assert.equal(nodes(tree).some(node => node.type === 'commit-input-row'), false);
  assert.equal(app.saves.length, 0, 'nothing saves until Save is pressed');
  assert.equal(nodes(tree).some(node => node.type === 'select' && node.props.value === 'claude'), true, 'the provider select is the original one');
  app.hooks.unmount();
});
