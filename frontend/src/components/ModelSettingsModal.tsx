import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import { Meta, SettingsFooter, SettingsRow, StatusText } from '@/components/ui/settings-layout';
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue } from '@/components/ui/select';
import { useSummaryConfig, type KeyProvider } from '@/contexts/ConfigContext';
import { ROW_SELECT } from '@/components/settings/settings-parts';
import { providerLabel } from '@/components/settings/provider-data';
import { SaveMark, withError } from '@/components/settings/summary-provider-parts';
import { SaveError, useSaveFeedback, type CommitResult } from '@/components/settings/commit-field';
import { NO_SECRET, keyProblem, newKeyPayload, readSecretStatus, removalNeedsConfirmation, withoutSecrets } from '@/lib/secret-key';
import type { ModelConfig } from '@/services/configService';
import { BuiltInSettings, useBuiltInModels } from './model-settings/BuiltInSettings';
import { OllamaEndpoint, OllamaSettings, useOllamaSettings } from './model-settings/OllamaSettings';
import { HostedProviderSettings, OpenAIKeyLink, isHostedProvider, saveOpenAIAuthMode, useHostedProviderSettings } from './model-settings/HostedProviderSettings';
import { CompatibleOpenAIAdvanced, CompatibleOpenAISettings, DEFAULT_COMPATIBLE_MODEL, useCompatibleOpenAISettings } from './model-settings/CompatibleOpenAISettings';
import { CodexAdvanced, CodexSettings, useCodexSettings } from './model-settings/CodexSettings';
import { CLAUDE_CODE_MODELS, ClaudeCodeAdvanced, ClaudeCodeSettings, useClaudeCodeProvider } from './model-settings/ClaudeCodeSettings';
import { readModelMap, rememberModel, type ProviderView, type UpdateConfig } from './model-settings/shared';

export type { ModelConfig } from '@/services/configService';

interface ModelSettingsModalProps {
  modelConfig: ModelConfig;
  setModelConfig: UpdateConfig;
  onSave: (config: ModelConfig) => void | Promise<void>;
  skipInitialFetch?: boolean;
  layout?: 'inline' | 'dialog';
  /** Settings commits individual changes. The meeting dialog saves on demand. */
  autosave?: boolean;
}

const PROVIDER_SUMMARY: Record<ModelConfig['provider'], string> = {
  'builtin-ai': 'Runs on this PC, offline, no key',
  ollama: 'Uses your local Ollama server',
  'custom-openai': 'OpenAI or any OpenAI-compatible gateway',
  claude: 'Anthropic API with your own key',
  openai: 'OpenAI API with your own key',
  groq: 'Groq API with your own key',
  openrouter: 'OpenRouter API with your own key',
  codex: 'Bundled Codex runtime signed in with ChatGPT',
  'claude-code': 'Uses the Claude Code you installed and signed in to',
};

export function ModelSettingsModal({
  modelConfig: propsModelConfig, setModelConfig: propsSetModelConfig, onSave,
  skipInitialFetch = false, layout = 'inline', autosave = false,
}: ModelSettingsModalProps) {
  const context = useSummaryConfig();
  const shared = context?.modelConfig ?? propsModelConfig;
  const setShared = context?.setModelConfig ?? propsSetModelConfig;
  const refreshKeyStatus = context?.refreshProviderKeyStatus;
  const [preview, setPreviewState] = useState<ModelConfig | null>(null);
  const previewRef = useRef<ModelConfig | null>(null);
  const setPreview = useCallback((next: ModelConfig | null) => {
    previewRef.current = next;
    setPreviewState(next);
  }, []);
  const updateConfig: UpdateConfig = useCallback((next) => {
    const previous = previewRef.current;
    if (typeof next === 'function') {
      if (previous) setPreview(next(previous));
      else setShared(next);
    } else if (previous?.provider === next.provider) {
      setPreview(next);
    } else {
      setShared(current => current.provider === next.provider ? next : current);
    }
  }, [setPreview, setShared]);

  const config = preview ?? shared;
  const { provider, model } = config;
  const [advancedProvider, setAdvancedProvider] = useState<ModelConfig['provider'] | null>(null);
  const feedback = useSaveFeedback();
  const savePending = useRef(false);
  const keyStatus = context?.providerKeyStatus?.[provider as KeyProvider] ?? config.apiKeyStatus ?? NO_SECRET;

  const saveConfig = async (next: ModelConfig, failure: string, switching = false): Promise<CommitResult> => {
    if (preview && !switching) return 'held';
    try { await onSave(next); }
    catch { throw new SaveError(failure); }
    if (switching) setPreview(null);
  };

  const hosted = useHostedProviderSettings(provider, keyStatus);
  const ollama = useOllamaSettings(provider === 'ollama', config.ollamaEndpoint ?? '');
  const compatible = useCompatibleOpenAISettings({ active: provider === 'custom-openai', modelConfig: config, updateConfig, saveConfig, refreshKeyStatus });
  const codexPreview = preview?.provider === 'codex';
  const codex = useCodexSettings(provider === 'codex', codexPreview, model, updateConfig);
  const claudeCode = useClaudeCodeProvider(provider === 'claude-code');
  const builtin = useBuiltInModels();
  const options = {
    ...hosted.options, 'builtin-ai': builtin.models, ollama: ollama.models.map(item => item.name),
    codex: codex.options.map(item => item.id), 'claude-code': CLAUDE_CODE_MODELS, 'custom-openai': [DEFAULT_COMPATIBLE_MODEL],
  };
  const currentOptions = options[provider] ?? [];

  useEffect(() => {
    if (skipInitialFetch) return;
    let current = true;
    void invoke<ModelConfig | null>('api_get_model_config').then(data => {
      if (current && data?.provider) setShared({ ...withoutSecrets(data), apiKeyStatus: readSecretStatus(data.apiKeyStatus) });
    }).catch(() => { console.error('Could not load model settings'); });
    return () => { current = false; };
  }, [skipInitialFetch, setShared]);

  useEffect(() => {
    if (provider === 'custom-openai' || (provider === 'builtin-ai' && !model) || currentOptions.includes(model)) return;
    const cached = readModelMap()[provider];
    if (cached && currentOptions.includes(cached)) updateConfig(prev => prev.provider === provider ? { ...prev, model: cached } : prev);
  }, [currentOptions, model, provider, updateConfig]);

  const changeProvider = (value: string) => {
    const selected = value as ModelConfig['provider'];
    if (selected === provider) return;
    setAdvancedProvider(null);
    if (autosave && selected === shared.provider) { setPreview(null); return; }
    if (!autosave && model) rememberModel(provider, model);
    const saved = readModelMap()[selected];
    const models = options[selected] ?? [];
    const next = {
      ...withoutSecrets(autosave ? shared : config), provider: selected,
      model: models.includes(saved) ? saved : models[0] ?? '',
      apiKeyStatus: context?.providerKeyStatus?.[selected as KeyProvider],
    };
    if (autosave) setPreview(next);
    else setShared(next);
    if (selected === 'builtin-ai') void builtin.load();
  };

  const missing = () => {
    if (provider === 'custom-openai' && !compatible.ready) return compatible.load === 'error'
      ? 'Could not load the compatible provider settings' : 'Checking compatible provider settings…';
    if (isHostedProvider(provider) && !hosted.usable) return 'Enter an API key first';
    if (provider !== 'codex' && provider !== 'claude-code' && provider !== 'custom-openai' && !model.trim()) return 'Choose a model first';
    if (provider === 'claude-code' && claudeCode.status && claudeCode.status.state !== 'ready') return 'Claude Code is not ready yet';
    return null;
  };

  const submit = (switching: boolean) => {
    if (savePending.current) return;
    savePending.current = true;
    void feedback.run('provider', async () => {
      try {
        let next: ModelConfig = { ...config, apiKey: isHostedProvider(provider) ? newKeyPayload(hosted.key) : null, customOpenAIApiKey: null };
        if (!switching && provider === 'custom-openai') next = { ...next, ...await compatible.save() };
        if (!switching && provider === 'ollama') next.ollamaEndpoint = ollama.endpoint.trim() || null;
        if (!switching && provider === 'codex') {
          const saved = await codex.save({ ...codex.config, model: codex.config.model || model });
          next.model = saved.model;
        }
        if (!switching && provider === 'claude-code') {
          try { await claudeCode.save(); } catch { throw new SaveError('Could not save Claude Code settings'); }
        }
        if (provider === 'openai') await saveOpenAIAuthMode();
        await saveConfig(next, switching ? 'Could not save the provider' : 'Could not save model settings', switching);
        if (switching && shared.model) rememberModel(shared.provider, shared.model);
        if (next.model) rememberModel(provider, next.model);
        setShared({ ...next, apiKey: null });
        if (isHostedProvider(provider)) hosted.markKeyChanged();
        await refreshKeyStatus?.(provider);
      } finally { savePending.current = false; }
    });
  };

  const chooseModel = (selected: string) => {
    if (selected === model) return;
    if (!autosave) {
      if (provider === 'codex') codex.setConfig({ ...codex.config, model: selected });
      updateConfig({ ...config, model: selected });
      return;
    }
    void feedback.run('model', async () => {
      if (provider === 'codex') await codex.save({ ...codex.config, model: selected });
      const next = { ...config, model: selected, apiKey: null };
      const held = await saveConfig(next, 'Could not save the model');
      rememberModel(provider, selected);
      updateConfig(next);
      return held;
    });
  };

  const commitKey = async (value: string) => {
    await saveConfig({ ...config, apiKey: newKeyPayload(value) }, 'Could not save the API key');
    hosted.markKeyChanged();
    await refreshKeyStatus?.(provider);
    if (provider === 'openai') await saveOpenAIAuthMode();
  };
  const removeKey = async () => {
    await hosted.remove();
    updateConfig(previous => ({ ...previous, apiKey: null, apiKeyStatus: NO_SECRET }));
    await refreshKeyStatus?.(provider);
  };
  const removeWarning = removalNeedsConfirmation(provider, shared.provider)
    ? 'Summaries with this provider will not work until you add a key again.' : null;

  const providerState = feedback.states.provider;
  const busy = feedback.busy || (provider === 'custom-openai' && compatible.saving);
  const modelControls = { model, chooseModel, disabled: busy, feedback: feedback.states.model };
  let view: ProviderView;
  if (isHostedProvider(provider)) view = {
    content: <HostedProviderSettings {...modelControls} provider={provider} options={currentOptions} loading={hosted.loading} autosave={autosave}
      preview={!!preview} keyStatus={keyStatus} draft={hosted.key} setDraft={hosted.setKey} commit={commitKey} remove={removeKey} removeWarning={removeWarning} />,
    advanced: provider === 'openai' && <OpenAIKeyLink />,
    status: hosted.status, check: hosted.check, checking: hosted.loading,
    checkDisabled: provider !== 'openrouter' && (!hosted.usable || !!keyProblem(hosted.key)),
  };
  else if (provider === 'builtin-ai') view = {
    content: <BuiltInSettings model={model} chooseModel={chooseModel} layout="inline" feedback={feedback.states.model} />,
    status: model ? { tone: 'ok', text: 'Runs on this PC', meta: 'Offline' } : { tone: 'neutral', text: 'No summary model selected' },
  };
  else if (provider === 'ollama') view = {
    content: <OllamaSettings {...modelControls} ollama={ollama} />,
    advanced: <OllamaEndpoint ollama={ollama} autosave={autosave} commit={async value => {
      const next = { ...config, apiKey: null, ollamaEndpoint: value || null };
      const held = await saveConfig(next, 'Could not save the endpoint');
      updateConfig(next);
      return held;
    }} />,
    status: ollama.status, check: ollama.check, checking: ollama.loading,
  };
  else if (provider === 'custom-openai') view = {
    content: <CompatibleOpenAISettings state={compatible} autosave={autosave} removeWarning={removeWarning} />,
    advanced: compatible.ready && <CompatibleOpenAIAdvanced state={compatible} autosave={autosave} />,
    status: compatible.ready ? compatible.status : undefined, check: compatible.ready ? () => compatible.runTest() : undefined,
    checkLabel: compatible.connection.state === 'idle' && compatible.catalog.status === 'idle' ? 'Check connection' : 'Check again',
    checking: compatible.checking, checkDisabled: compatible.saving,
  };
  else if (provider === 'codex') view = {
    content: <CodexSettings {...modelControls} state={codex} preview={codexPreview} />,
    advanced: <CodexAdvanced state={codex} autosave={autosave} preview={codexPreview} />,
    status: codex.status, check: codex.check, checking: codex.busy, checkDisabled: codexPreview,
  };
  else view = {
    content: <ClaudeCodeSettings {...modelControls} state={claudeCode} autosave={autosave} />,
    advanced: autosave && <ClaudeCodeAdvanced state={claudeCode} />,
    status: autosave ? claudeCode.health : undefined, check: autosave ? claudeCode.check : undefined, checking: claudeCode.busy,
  };

  const noSummaryModel = shared.provider === 'builtin-ai' && !shared.model;
  const incomplete = preview ? missing() : null;
  const advancedOpen = advancedProvider === provider;
  const dialogSaveBlocked = busy || (isHostedProvider(provider) && (!hosted.usable || !!keyProblem(hosted.key)))
    || (provider === 'custom-openai' && (compatible.blocked || compatible.connection.state === 'checking'))
    || (provider === 'ollama' && ollama.needsCheck && ollama.endpoint.trim() !== (config.ollamaEndpoint ?? '').trim())
    || (provider === 'claude-code' && claudeCode.busy)
    || (provider === 'codex' && codex.busy);

  const content = <>
    {!autosave && <h3 className="px-5 pb-2 text-lg font-semibold">Model settings</h3>}
    <SettingsRow title="Provider" htmlFor="summary-provider"
      description={withError(PROVIDER_SUMMARY[provider], providerState?.status === 'error' ? providerState.message : null)}>
      <SaveMark status={providerState?.status} />
      <Select value={provider} onValueChange={changeProvider} disabled={busy}>
        <SelectTrigger id="summary-provider" aria-label="Provider" className={ROW_SELECT}>
          <SelectValue placeholder="Select a provider" />
        </SelectTrigger>
        <SelectContent className="max-h-72">
          <SelectGroup>
            <SelectLabel className="normal-case">On your device</SelectLabel>
            <SelectItem value="builtin-ai">Built-in</SelectItem>
            <SelectItem value="ollama">Ollama</SelectItem>
          </SelectGroup>
          <SelectGroup>
            <SelectLabel className="normal-case">Cloud APIs</SelectLabel>
            {shared.provider === 'openai' && <SelectItem value="openai">OpenAI API</SelectItem>}
            <SelectItem value="custom-openai">OpenAI or compatible</SelectItem>
            <SelectItem value="claude">Claude</SelectItem>
            <SelectItem value="groq">Groq</SelectItem>
            <SelectItem value="openrouter">OpenRouter</SelectItem>
          </SelectGroup>
          <SelectGroup>
            <SelectLabel className="normal-case">Advanced</SelectLabel>
            <SelectItem value="codex">Codex app-server</SelectItem>
            <SelectItem value="claude-code">Claude Code</SelectItem>
          </SelectGroup>
        </SelectContent>
      </Select>
    </SettingsRow>
    {preview && <SettingsRow title="Not in use yet" description={incomplete ?? (noSummaryModel
      ? 'Choose Use to save your summary provider.' : `${providerLabel(shared.provider)} stays your summary provider until you switch`)}>
      <Button type="button" variant="ghost" size="sm" disabled={busy} onClick={() => changeProvider(shared.provider)}>
        {noSummaryModel ? 'Cancel' : `Keep ${providerLabel(shared.provider)}`}
      </Button>
      <Button type="button" variant="outline" size="sm" disabled={busy || incomplete !== null} onClick={() => submit(true)}>
        Use {providerLabel(provider)}
      </Button>
    </SettingsRow>}
    {view.content}
    {view.advanced && <div id="summary-provider-advanced" hidden={!advancedOpen} className="divide-y divide-border/60">
      {advancedOpen && view.advanced}
    </div>}
    <SettingsFooter actions={<>
      {view.advanced && <Button type="button" variant="ghost" size="sm" aria-expanded={advancedOpen} aria-controls="summary-provider-advanced"
        onClick={() => setAdvancedProvider(advancedOpen ? null : provider)}>Advanced</Button>}
      {view.check && <Button type="button" variant="ghost" size="sm" disabled={busy || view.checking || view.checkDisabled}
        onClick={() => void view.check?.()}>{view.checking ? 'Checking…' : view.checkLabel ?? 'Check again'}</Button>}
      {!autosave && <Button type="submit" disabled={dialogSaveBlocked}>Save</Button>}
    </>}>
      {view.status && <span role="status"><StatusText tone={view.status.tone} className="whitespace-normal">{view.status.text}</StatusText></span>}
      {view.status?.meta && <Meta>{view.status.meta}</Meta>}
    </SettingsFooter>
  </>;
  if (autosave) return content;
  return <form className={layout === 'dialog' ? 'max-h-[85dvh] min-w-0 overflow-y-auto px-1' : undefined}
    onSubmit={event => { event.preventDefault(); if (!dialogSaveBlocked) submit(false); }}>
    {content}
  </form>;
}
