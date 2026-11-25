import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ExternalLink } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { SettingsRow } from '@/components/ui/settings-layout';
import { SecretKeyRow, SecretKeyControl } from '@/components/settings/SecretKeyControl';
import { SaveError, type CommitResult, type FeedbackState } from '@/components/settings/commit-field';
import { configService, type ModelConfig } from '@/services/configService';
import { hasUsableKey, keyProblem, newKeyPayload, type SecretStatus } from '@/lib/secret-key';
import { openExternal } from '@/lib/openExternal';
import { ModelPicker } from './shared';
import { catalogStatus, useModelCatalog } from './useModelCatalog';

const FALLBACK_MODELS = {
  openai: ['gpt-6.1-sol', 'gpt-6-sol', 'gpt-6-luna', 'gpt-6-astra', 'gpt-5.6-sol', 'gpt-5.6-luna', 'gpt-5.5', 'gpt-4o-mini'],
  claude: ['claude-opus-5', 'claude-sonnet-5', 'claude-haiku-4-5', 'claude-fable-5-1'],
  groq: ['llama-3.3-70b-versatile', 'openai/gpt-oss-120b', 'openai/gpt-oss-20b'],
  openrouter: [],
};

const CATALOG_COMMANDS = { openai: 'get_openai_models', claude: 'get_anthropic_models', groq: 'get_groq_models' } as const;
type HostedProvider = keyof typeof FALLBACK_MODELS;
export const isHostedProvider = (provider: ModelConfig['provider']): provider is HostedProvider =>
  Object.hasOwn(FALLBACK_MODELS, provider);

export async function saveOpenAIAuthMode() {
  try { await invoke('api_save_openai_auth_config', { config: { mode: 'api_key' } }); }
  catch { throw new SaveError('Could not save the OpenAI sign-in mode'); }
}

export function useHostedProviderSettings(provider: ModelConfig['provider'], keyStatus: SecretStatus) {
  const [draft, setDraft] = useState({ provider, value: '', revision: 0 });
  const [keyRevision, setKeyRevision] = useState(0);
  const [publicCatalog, setPublicCatalog] = useState<{ models: string[]; loading: boolean; failed: boolean }>({
    models: [], loading: false, failed: false,
  });
  const key = draft.provider === provider ? draft.value : '';
  const draftRevision = draft.provider === provider ? draft.revision : 0;
  const setKey = (value: string) => {
    setDraft(previous => ({ provider, value, revision: previous.revision + 1 }));
  };
  useEffect(() => {
    setDraft(previous => previous.provider === provider ? previous : { provider, value: '', revision: 0 });
  }, [provider]);

  const keyedProvider = provider === 'openai' || provider === 'claude' || provider === 'groq' ? provider : undefined;
  const usable = hasUsableKey(key, keyStatus);
  // Catalogs can load automatically with a stored key. Drafts require an explicit check.
  const auto = !!keyedProvider && keyStatus.stored && (provider === 'openai' ? !key.trim() : draftRevision === 0);
  const scope = `${provider}:${keyStatus.stored}:${keyRevision}:${draftRevision}`;
  const { catalog, refresh } = useModelCatalog(keyedProvider && CATALOG_COMMANDS[keyedProvider], scope, auto);

  const loadPublicCatalog = useCallback(async () => {
    setPublicCatalog(previous => ({ ...previous, loading: true, failed: false }));
    try {
      const rows = await invoke<{ id: string }[]>('get_openrouter_models');
      setPublicCatalog({ models: rows.map(model => model.id), loading: false, failed: false });
    } catch { setPublicCatalog({ models: [], loading: false, failed: true }); }
  }, []);
  useEffect(() => {
    if (provider === 'openrouter') void loadPublicCatalog();
  }, [provider, loadPublicCatalog]);

  const check = async () => {
    if (provider === 'openrouter') return loadPublicCatalog();
    if (usable && !keyProblem(key)) await refresh(newKeyPayload(key));
  };
  const markKeyChanged = () => {
    setDraft({ provider, value: '', revision: 0 });
    setKeyRevision(revision => revision + 1);
  };
  const remove = async () => {
    await configService.clearApiKey(provider);
    markKeyChanged();
  };
  const options = {
    ...FALLBACK_MODELS,
    ...(keyedProvider && catalog.models.length ? { [keyedProvider]: catalog.models } : {}),
    openrouter: publicCatalog.models,
  };
  const loading = provider === 'openrouter' ? publicCatalog.loading : catalog.status === 'loading';
  const status = !usable ? { tone: 'warning', text: 'Needs an API key' } as const
    : loading ? { tone: 'neutral', text: 'Loading models…' } as const
    : provider === 'openrouter' ? publicCatalog.failed
      ? { tone: 'warning', text: 'Public model list unavailable' } as const
      : { tone: 'neutral', text: 'API key not verified by the public model list' } as const
    : catalogStatus(catalog.status);

  return { options, key, setKey, markKeyChanged, remove, usable, loading, status, check };
}

export function HostedProviderSettings({ provider, model, options, loading, chooseModel, feedback, disabled, autosave, preview, keyStatus, draft, setDraft, commit, remove, removeWarning }: {
  provider: HostedProvider; model: string; options: string[]; loading: boolean; chooseModel: (model: string) => void;
  feedback?: FeedbackState; disabled: boolean; autosave: boolean; preview: boolean; keyStatus: SecretStatus;
  draft: string; setDraft: (key: string) => void; commit: (key: string) => Promise<CommitResult>;
  remove: () => Promise<void>; removeWarning: string | null;
}) {
  return <>
    {autosave ? (
      preview ? <SecretKeyRow key={provider} id="summary-api-key" title="API key" status={keyStatus} draft={draft} onDraftChange={setDraft}
        description="Protected by your operating system. It is saved when you use this provider" />
        : <SecretKeyRow key={provider} id="summary-api-key" title="API key" status={keyStatus} onCommit={commit} onRemove={remove} removeWarning={removeWarning}
          description={provider === 'openai' ? 'For ChatGPT sign-in, choose Codex app-server' : 'Protected by your operating system'} />
    ) : <SettingsRow title="API key" htmlFor="legacy-api-key">
      <SecretKeyControl key={provider} id="legacy-api-key" status={keyStatus} draft={draft} onDraftChange={setDraft}
        onRemove={remove} removeWarning={removeWarning} />
    </SettingsRow>}
    <ModelPicker value={model} options={options} onChoose={chooseModel} loading={loading} feedback={feedback} disabled={disabled} />
  </>;
}

export function OpenAIKeyLink() {
  return <SettingsRow title="Get an API key" description="Opens the OpenAI platform in your browser">
    <Button type="button" variant="outline" onClick={() => void openExternal('https://platform.openai.com/api-keys')}>
      <ExternalLink aria-hidden="true" />Open API keys
    </Button>
  </SettingsRow>;
}
