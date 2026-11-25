import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { RefreshCw } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { SettingsRow } from '@/components/ui/settings-layout';
import { UnencryptedHttpOptIn } from '@/components/UnencryptedHttpOptIn';
import { NoteRow } from '@/components/settings/settings-parts';
import { SecretKeyControl, SecretKeyRow } from '@/components/settings/SecretKeyControl';
import { SaveError, safeMessage, useSaveFeedback, type CommitResult } from '@/components/settings/commit-field';
import { configService, customOpenAIModelFields, type CustomOpenAIConfig, type ModelConfig } from '@/services/configService';
import { KEY_REQUIRED_FOR_NEW_ADDRESS, NO_SECRET, hasUsableKey, keepsAddress, keyProblem, keyRequiredForAddress, newKeyPayload } from '@/lib/secret-key';
import { needsHttpOptIn, secretDestinationError } from '@/lib/secretDestination';
import { ModelPicker, ProviderTextField, numberProblem, rememberModel, requiredProblem, urlProblem, type CommitConfig, type ProviderStatus, type UpdateConfig } from './shared';
import { catalogStatus, useModelCatalog } from './useModelCatalog';

const DEFAULT_ENDPOINT = 'https://api.openai.com/v1';
export const DEFAULT_COMPATIBLE_MODEL = 'gpt-4o-mini';

export function isOfficialOpenAIEndpoint(value: string): boolean {
  try {
    const url = new URL(value);
    return url.origin === 'https://api.openai.com' && !url.username && !url.password && !url.search && !url.hash
      && ['', '/v1', '/v1/chat/completions', '/v1/responses'].includes(url.pathname.replace(/\/+$/, ''));
  } catch { return false; }
}

function readFields(config: CustomOpenAIConfig | null) {
  return {
    endpoint: config?.endpoint || DEFAULT_ENDPOINT,
    model: config?.model || DEFAULT_COMPATIBLE_MODEL,
    apiKey: '',
    allowUnencrypted: config?.allow_unencrypted ?? false,
    contextWindow: config?.contextWindow?.toString() || '8192',
    maxTokens: config?.maxTokens?.toString() || '',
    temperature: config?.temperature?.toString() || '',
    topP: config?.topP?.toString() || '',
    timeoutSeconds: config?.timeoutSeconds?.toString() ?? '',
    organization: config?.organization || '',
    project: config?.project || '',
  };
}
type Fields = ReturnType<typeof readFields>;
type CustomCheck = { state: 'idle' | 'checking' | 'ok' | 'error'; message?: string };

function requestConfig(fields: Fields) {
  return {
    endpoint: fields.endpoint.trim(),
    model: fields.model.trim(),
    apiKey: newKeyPayload(fields.apiKey),
    allowUnencrypted: fields.allowUnencrypted,
    contextWindow: Number(fields.contextWindow) || 8192,
    maxTokens: fields.maxTokens ? Number(fields.maxTokens) : null,
    temperature: fields.temperature ? Number(fields.temperature) : null,
    topP: fields.topP ? Number(fields.topP) : null,
    timeoutSeconds: fields.timeoutSeconds ? Number(fields.timeoutSeconds) : null,
    organization: fields.organization.trim() || null,
    project: fields.project.trim() || null,
  };
}

function modelFields(payload: ReturnType<typeof requestConfig>, keyStatus: CustomOpenAIConfig['apiKeyStatus']) {
  return customOpenAIModelFields({ ...payload, apiKeyStatus: keyStatus, allow_unencrypted: payload.allowUnencrypted, destination_problem: null });
}

export function useCompatibleOpenAISettings({ active, modelConfig, updateConfig, saveConfig, refreshKeyStatus }: {
  active: boolean; modelConfig: ModelConfig; updateConfig: UpdateConfig; saveConfig: CommitConfig;
  refreshKeyStatus?: (provider: string) => Promise<unknown>;
}) {
  const [load, setLoad] = useState<'loading' | 'ready' | 'error'>('loading');
  const [attempt, setAttempt] = useState(0);
  const [fields, setFields] = useState(() => readFields(null));
  const [savedEndpoint, setSavedEndpoint] = useState('');
  const [keyStatus, setKeyStatus] = useState(NO_SECRET);
  const [destinationProblem, setDestinationProblem] = useState<string | null>(null);
  const [keyRevision, setKeyRevision] = useState(0);
  const [connection, setConnection] = useState<CustomCheck>({ state: 'idle' });
  const [processing, setProcessing] = useState(false);
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const checkPending = useRef(false);
  const feedback = useSaveFeedback();

  useEffect(() => {
    setLoad('loading');
    setConnection({ state: 'idle' });
    if (!active) return;
    let current = true;
    void configService.getCustomOpenAIConfig().then(config => {
      if (!current) return;
      const next = readFields(config);
      setFields(next);
      setSavedEndpoint(next.endpoint);
      setKeyStatus(config?.apiKeyStatus ?? NO_SECRET);
      setDestinationProblem(config?.destination_problem ?? null);
      updateConfig(prev => prev.provider === 'custom-openai'
        ? { ...prev, ...customOpenAIModelFields(config), model: next.model } : prev);
      setLoad('ready');
    }).catch(() => { if (current) setLoad('error'); });
    return () => { current = false; };
  }, [active, attempt, updateConfig]);

  const ready = active && load === 'ready';
  const endpoint = fields.endpoint.trim();
  const official = ready && isOfficialOpenAIEndpoint(endpoint);
  const sameEndpoint = keepsAddress(savedEndpoint, endpoint) && keepsAddress(endpoint, savedEndpoint);
  const keyRequired = keyRequiredForAddress(savedEndpoint, endpoint, keyStatus, fields.apiKey);
  const credentialed = hasUsableKey(fields.apiKey, keyStatus);
  const scope = official ? `${endpoint}:${keyRevision}:${keyStatus.stored}:${sameEndpoint}` : '';
  const { catalog, refresh } = useModelCatalog(official ? 'get_openai_models' : undefined, scope,
    official && sameEndpoint && keyStatus.stored && !fields.apiKey.trim(), null, official ? endpoint : undefined);

  const retry = () => { setLoad('loading'); setAttempt(value => value + 1); };
  const draft = (next: Partial<Fields>) => {
    setFields(previous => ({ ...previous, ...next }));
    setConnection({ state: 'idle' });
    if ('apiKey' in next) setKeyRevision(revision => revision + 1);
    if ('endpoint' in next || 'allowUnencrypted' in next) setDestinationProblem(null);
  };
  const updateKeyStatus = async () => {
    let status = NO_SECRET;
    try {
      const config = await configService.getCustomOpenAIConfig();
      status = config?.apiKeyStatus ?? NO_SECRET;
      setKeyStatus(status);
      setKeyRevision(revision => revision + 1);
    } catch { setKeyStatus(NO_SECRET); }
    await refreshKeyStatus?.('custom-openai');
    return status;
  };

  const persist = async (values: Fields) => {
    const payload = requestConfig(values);
    try { await invoke('api_save_custom_openai_config', payload); }
    catch (error) {
      throw new SaveError(safeMessage(error, 'Could not save the connection settings', [values.apiKey, values.organization, values.project]));
    }
    const savedStatus = payload.apiKey ? await updateKeyStatus() : keyStatus;
    setSavedEndpoint(payload.endpoint);
    setDestinationProblem(null);
    setFields(readFields({ ...payload, allow_unencrypted: payload.allowUnencrypted, apiKeyStatus: savedStatus }));
    return { ...modelFields(payload, savedStatus), model: payload.model, customOpenAIApiKey: null };
  };

  const commit = async (changes: Partial<Fields>): Promise<CommitResult> => {
    if (!ready) throw new SaveError('Could not load the compatible provider settings');
    if (savingRef.current) throw new SaveError('A connection setting is already being saved. Try again');
    const next = { ...fields, ...changes };
    if (keyRequiredForAddress(savedEndpoint, next.endpoint, keyStatus, next.apiKey)) {
      draft(changes);
      return 'held';
    }
    savingRef.current = true;
    setSaving(true);
    try {
      const saved = await persist(next);
      const config: ModelConfig = { ...modelConfig, ...saved, provider: 'custom-openai', apiKey: null };
      const held = await saveConfig(config, 'Could not save the model settings');
      updateConfig(config);
      rememberModel('custom-openai', config.model);
      setConnection({ state: 'idle' });
      return held;
    } finally {
      savingRef.current = false;
      setSaving(false);
    }
  };

  const blocked = !ready || !endpoint || !fields.model.trim() || keyRequired || !!keyProblem(fields.apiKey);
  const save = async () => {
    if (blocked) throw new SaveError('Complete the connection settings before saving');
    return persist(fields);
  };

  const checkProblem = () => {
    if (!ready || !endpoint || !fields.model.trim()) return 'Enter an API base URL and a model name first';
    if (keyRequired) return KEY_REQUIRED_FOR_NEW_ADDRESS;
    if (keyProblem(fields.apiKey)) return keyProblem(fields.apiKey);
    if (needsHttpOptIn(endpoint, credentialed) && !fields.allowUnencrypted) {
      return 'Allow unencrypted HTTP for this endpoint or use HTTPS';
    }
    return secretDestinationError(endpoint, fields.allowUnencrypted, credentialed);
  };

  const runTest = async (processingTest = false) => {
    if (checkPending.current || savingRef.current) return;
    const problem = checkProblem();
    if (problem) {
      setConnection({ state: 'error', message: problem });
      return;
    }
    if (!processingTest && official) {
      if (credentialed) await refresh(newKeyPayload(fields.apiKey));
      else setConnection({ state: 'error', message: 'Enter an API key first' });
      return;
    }

    checkPending.current = true;
    setConnection({ state: 'checking' });
    setProcessing(processingTest);
    try {
      const command = processingTest ? 'api_test_custom_openai_processing' : 'api_test_custom_openai_connection';
      await invoke(command, requestConfig(fields));
      setConnection({ state: 'ok' });
      if (processingTest) toast.success('Test meeting processed successfully');
    } catch {
      setConnection({ state: 'error', message: processingTest ? 'Could not process a test meeting' : 'Could not connect. Check the endpoint and model settings' });
    } finally {
      checkPending.current = false;
      setProcessing(false);
    }
  };

  const removeKey = async () => {
    await configService.clearCustomOpenAIApiKey();
    setKeyStatus(NO_SECRET);
    draft({ apiKey: '' });
    await refreshKeyStatus?.('custom-openai');
  };

  const status: ProviderStatus = connection.state === 'checking' ? { tone: 'neutral', text: 'Checking connection…' }
    : connection.state === 'error' ? { tone: 'error', text: 'Could not connect' }
    : official && catalog.status !== 'idle' ? catalogStatus(catalog.status, 'Enter a model name manually.')
    : connection.state === 'ok' ? { tone: 'ok', text: 'Connected' }
    : { tone: 'neutral', text: 'Not checked yet' };

  return {
    load, ready, retry, fields, draft, commit, save, blocked, keyStatus, keyRequired, credentialed, removeKey, destinationProblem,
    catalog, connection, processing, saving, checking: connection.state === 'checking' || catalog.status === 'loading',
    status, runTest, feedback,
  };
}

type CompatibleState = ReturnType<typeof useCompatibleOpenAISettings>;

export function CompatibleOpenAISettings({ state, autosave, removeWarning }: {
  state: CompatibleState; autosave: boolean; removeWarning: string | null;
}) {
  if (!state.ready) return <div className="px-5 py-3 text-sm text-muted-foreground">
    {state.load === 'error' ? <div className="flex items-center gap-2">
      <span role="status">Could not load the compatible provider settings.</span>
      <Button type="button" variant="outline" onClick={state.retry}>Retry</Button>
    </div> : <p role="status">Checking compatible provider settings…</p>}
  </div>;

  const { fields, commit, draft, keyStatus, keyRequired, credentialed, removeKey, destinationProblem, catalog, connection, saving } = state;
  const disabled = saving || connection.state === 'checking';
  const handleModel = (model: string) => {
    if (autosave) void state.feedback.run('catalog-model', () => commit({ model }));
    else draft({ model });
  };
  return <>
    <ProviderTextField id="custom-endpoint" title="API base URL" type="url" placeholder={DEFAULT_ENDPOINT}
      description="Use another /v1 endpoint for OpenAI-compatible gateways" value={fields.endpoint}
      validate={value => value ? urlProblem(value) : 'The API base URL is required'} disabled={disabled}
      onCommit={value => commit({ endpoint: value })} onDraft={autosave ? undefined : value => draft({ endpoint: value })} />
    <div className="px-5 py-3 empty:hidden">
      <UnencryptedHttpOptIn destinationProblem={destinationProblem} urls={[fields.endpoint]} checked={fields.allowUnencrypted}
        includeLoopback={credentialed} disabled={disabled} onChange={value => {
          if (autosave) void state.feedback.run('custom-http', () => commit({ allowUnencrypted: value }));
          else draft({ allowUnencrypted: value });
        }} />
      {state.feedback.states['custom-http']?.status === 'error' &&
        <p role="alert" className="text-sm text-error-foreground">{state.feedback.states['custom-http'].message}</p>}
    </div>
    <ProviderTextField id="custom-model" title="Model name" placeholder={DEFAULT_COMPATIBLE_MODEL}
      description="Enter a model name or choose one from the OpenAI catalog when available" value={fields.model}
      validate={requiredProblem('The model name')} disabled={disabled}
      onCommit={value => commit({ model: value })} onDraft={autosave ? undefined : value => draft({ model: value })} />
    {catalog.status === 'ready' && <ModelPicker id="openai-catalog-model" title="OpenAI catalog" value={fields.model}
      options={catalog.models} onChoose={handleModel} disabled={disabled} feedback={state.feedback.states['catalog-model']} />}
    {autosave ? <SecretKeyRow id="custom-api-key" title="API key" status={keyStatus} placeholder="Leave empty if not required"
      description="Protected by your operating system" onCommit={value => commit({ apiKey: value })} disabled={disabled}
      required={keyRequired ? KEY_REQUIRED_FOR_NEW_ADDRESS : null} onRemove={removeKey} removeWarning={removeWarning} />
      : <SettingsRow title="API key" htmlFor="custom-api-key">
        <SecretKeyControl id="custom-api-key" status={keyStatus} draft={fields.apiKey} disabled={disabled}
          onDraftChange={value => draft({ apiKey: value })} required={keyRequired ? KEY_REQUIRED_FOR_NEW_ADDRESS : null}
          onRemove={removeKey} removeWarning={removeWarning} />
      </SettingsRow>}
    {connection.state === 'error' && <NoteRow tone="error">{connection.message}</NoteRow>}
  </>;
}

const ADVANCED_FIELDS = [
  { name: 'timeoutSeconds', id: 'timeout', title: 'Timeout', description: 'Seconds, 5 or more', placeholder: '300', validate: numberProblem(5, 86400, true, true), inputMode: 'numeric' },
  { name: 'organization', id: 'organization', title: 'OpenAI organization', description: 'Optional', placeholder: 'org_...' },
  { name: 'project', id: 'project', title: 'OpenAI project', description: 'Optional', placeholder: 'proj_...' },
  { name: 'contextWindow', id: 'context-window', title: 'Context window', description: 'Tokens. Longer meetings are summarized in sections to fit', validate: numberProblem(4096, 2000000, true, true), inputMode: 'numeric' },
  { name: 'maxTokens', id: 'max-tokens', title: 'Maximum output tokens', description: 'Optional', placeholder: 'For example 4096', validate: numberProblem(1, 2000000, true), inputMode: 'numeric' },
  { name: 'temperature', id: 'temperature', title: 'Temperature', description: '0 to 2, optional', placeholder: 'For example 0.7', validate: numberProblem(0, 2, false) },
  { name: 'topP', id: 'top-p', title: 'Top P', description: '0 to 1, optional', placeholder: 'For example 0.9', validate: numberProblem(0, 1, false) },
] as const;

export function CompatibleOpenAIAdvanced({ state, autosave }: { state: CompatibleState; autosave: boolean }) {
  const { fields, commit, draft, runTest, saving, checking, processing, feedback, removeKey } = state;
  return <>
    {ADVANCED_FIELDS.map(({ name, id, ...props }) => <ProviderTextField key={id} id={`custom-${id}`} {...props}
      disabled={saving || checking} value={fields[name]} onCommit={value => commit({ [name]: value })}
      onDraft={autosave ? undefined : value => draft({ [name]: value })} />)}
    <SettingsRow title="Tests and credentials" description="Test a full meeting summary, or remove the saved key, organization and project">
      <Button type="button" variant="outline" disabled={saving || checking || state.blocked} onClick={() => void runTest(true)}>
        {processing && <RefreshCw aria-hidden="true" className="animate-spin motion-reduce:animate-none" />}Test meeting processing
      </Button>
      <Button type="button" variant="outline" disabled={saving || checking || feedback.busy} onClick={() => {
        void feedback.run('custom-clear', async () => {
          try { await removeKey(); } catch { throw new SaveError('Could not remove the saved key'); }
          const cleared = { apiKey: '', organization: '', project: '' };
          if (autosave) return commit(cleared);
          draft(cleared);
          toast.success('The saved key was removed. Choose Save to clear the organization and project');
        });
      }}>Clear credentials</Button>
    </SettingsRow>
    {feedback.states['custom-clear']?.status === 'error' &&
      <NoteRow tone="error">{feedback.states['custom-clear'].message}</NoteRow>}
  </>;
}
