import { openExternal } from '@/lib/openExternal';
import { UnencryptedHttpOptIn } from '@/components/UnencryptedHttpOptIn';
import { useState, useEffect, useRef, useCallback, useMemo, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import { useOllamaDownload } from '@/contexts/OllamaDownloadContext';
import { BuiltInModelManager, type BuiltInModelInfo } from '@/components/BuiltInModelManager';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { useConfig } from '@/contexts/ConfigContext';
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Lock, RefreshCw, CheckCircle2, XCircle, ChevronDown, ChevronUp, Download, ExternalLink, Check, ChevronsUpDown, Wrench } from 'lucide-react';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from '@/components/ui/command';
import { cn, isOllamaNotInstalledError } from '@/lib/utils';
import { toast } from 'sonner';
import { ClaudeCodeProviderCard, CLAUDE_CODE_MODELS, useClaudeCodeSettings } from './ClaudeCodeProviderCard';
import { platform as osPlatform } from '@tauri-apps/plugin-os';
import { Progress } from '@/components/ui/progress';
import { Meta, SettingsFooter, SettingsRow, StatusText, type StatusTone } from '@/components/ui/settings-layout';
import { NoteRow, ROW_SELECT } from '@/components/settings/settings-parts';
import { SaveError, safeMessage, useSaveFeedback, type CommitResult } from '@/components/settings/commit-field';
import { CommitInputRow, CopyCommandRow, SaveMark, withError } from '@/components/settings/summary-provider-parts';
import { providerLabel as summaryProviderLabel } from '@/components/settings/provider-data';
import { SecretKeyControl, SecretKeyRow } from '@/components/settings/SecretKeyControl';
import { configService, type CustomOpenAIConfig } from '@/services/configService';
import {
  KEY_REQUIRED_FOR_NEW_ADDRESS, NO_SECRET, hasUsableKey, keyProblem, keyRequiredForAddress, newKeyPayload, readSecretStatus,
  removalNeedsConfirmation, withoutSecrets, type SecretStatus,
} from '@/lib/secret-key';
import type { KeyProvider } from '@/contexts/ConfigContext';

export interface ModelConfig {
  provider: 'ollama' | 'groq' | 'claude' | 'claude-code' | 'openai' | 'openrouter' | 'builtin-ai' | 'custom-openai' | 'codex';
  model: string;
  whisperModel: string;
  /** A key the user has just typed, held only until it is saved. Never loaded from the core. */
  apiKey?: string | null;
  /** Whether the provider has a stored key, with a short hint. */
  apiKeyStatus?: SecretStatus;
  ollamaEndpoint?: string | null;
  // Custom OpenAI fields
  customOpenAIEndpoint?: string | null;
  customOpenAIModel?: string | null;
  /** A key just typed for the custom provider, held only until it is saved. */
  customOpenAIApiKey?: string | null;
  customOpenAIKeyStatus?: SecretStatus;
  maxTokens?: number | null;
  contextWindow?: number | null;
  temperature?: number | null;
  topP?: number | null;
  timeoutSeconds?: number | null;
  organization?: string | null;
  project?: string | null;
}

interface OllamaModel {
  name: string;
  id: string;
  size: string;
  modified: string;
}

interface OpenRouterModel {
  id: string;
  name: string;
  context_length?: number;
  prompt_price?: string;
  completion_price?: string;
}

interface OpenAIAuthStatus {
  mode: 'disabled' | 'api_key' | 'oauth_pkce';
  configured: boolean;
  apiKeyPresent: boolean;
  oauthPkceConfigured: boolean;
  oauthBrowserLaunchReady: boolean;
  oauthDeviceFlowConfigured: boolean;
  canAuthenticateRequests: boolean;
  requiresUserAction: boolean;
  source: string;
  message: string;
  nextAction: string;
  requestAuthentication: string;
  authReferenceUrl: string;
  unsupportedReason?: string;
}

interface CodexProviderConfig {
  codexHomeMode: 'clawscribe-isolated' | 'existing-user-codex-session';
  codexHomePath?: string | null;
  useExistingUserCodexSession: boolean;
  codexBinaryPath?: string | null;
  model: string;
  timeoutSeconds: number;
}

interface CodexInstallationStatus {
  found: boolean;
  version?: string | null;
  path?: string | null;
  runtimeSha256?: string | null;
  runtimeSourcePackage?: string | null;
  runtimeSourceUrl?: string | null;
  runtimeKind: string;
  codexHome: string;
  codexHomeMode: 'clawscribe-isolated' | 'existing-user-codex-session';
  authStatus?: string | null;
  accountEmail?: string | null;
  planType?: string | null;
  rateLimitState?: string | null;
  desktopAppDetected?: boolean;
  installCommand?: string | null;
  message: string;
}

interface CodexCommandStatus {
  success: boolean;
  exitCode?: number | null;
  stdout: string;
  stderr: string;
  message: string;
}

interface CodexReasoningEffort {
  reasoningEffort: string;
  description?: string | null;
}

interface CodexModelInfo {
  id: string;
  displayName: string;
  hidden: boolean;
  isDefault: boolean;
  defaultReasoningEffort?: string | null;
  supportedReasoningEfforts: CodexReasoningEffort[];
  inputModalities: string[];
}

interface CodexInstallCommand {
  label: string;
  shell: string;
  command: string;
}

interface CodexInstallRepairPlan {
  requiresConfirmation: boolean;
  docsUrl: string;
  message: string;
  recommended: CodexInstallCommand;
  alternatives: CodexInstallCommand[];
}

const CODEX_DEFAULT_MODEL = 'gpt-5.6-sol';

// Fallback models for when API fetch fails or no API key provided
const OPENAI_FALLBACK_MODELS = [
  'gpt-6.1-sol',
  'gpt-6-sol',
  'gpt-6-luna',
  'gpt-6-astra',
  'gpt-5.6-sol',
  'gpt-5.6-luna',
  'gpt-5.5',
  'gpt-4o-mini',
];

const DEFAULT_OPENAI_COMPATIBLE_ENDPOINT = 'https://api.openai.com/v1';
const DEFAULT_OPENAI_COMPATIBLE_MODEL = 'gpt-4o-mini';
const DEFAULT_OPENAI_COMPATIBLE_TIMEOUT_SECONDS = '300';

const CLAUDE_FALLBACK_MODELS = [
  'claude-opus-5',
  'claude-sonnet-5',
  'claude-haiku-4-5',
  'claude-fable-5-1',
];

const GROQ_FALLBACK_MODELS = [
  'llama-3.3-70b-versatile',
  'openai/gpt-oss-120b',
  'openai/gpt-oss-20b',
];

const KEY_PROVIDERS = ['claude', 'groq', 'openai', 'openrouter'];
const needsKey = (provider: string) => KEY_PROVIDERS.includes(provider);
/** Providers that cannot summarize until a model is chosen from their list. */
const MODEL_LIST_PROVIDERS = ['builtin-ai', 'ollama', 'openrouter', 'claude', 'groq', 'openai'];

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

const urlProblem = (value: string): string | null => {
  try {
    const parsed = new URL(value);
    if (parsed.protocol === 'http:' || parsed.protocol === 'https:') return null;
  } catch { /* falls through */ }
  return 'Enter a URL that starts with http:// or https://';
};
const requiredProblem = (label: string) => (value: string) => (value.trim() ? null : `${label} is required`);
const numberProblem = (min: number, max: number, integer: boolean, required = false) => (value: string): string | null => {
  if (!value.trim()) return required ? 'This value is required' : null;
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || (integer && !Number.isInteger(parsed)) || parsed < min || parsed > max) {
    return `Enter ${integer ? 'a whole number' : 'a number'} from ${min} to ${max}`;
  }
  return null;
};
const readModelMap = (): Record<string, string> => {
  try { return JSON.parse(localStorage.getItem('providerModelMap') || '{}'); } catch { return {}; }
};
const rememberModel = (provider: string, model: string) => {
  try { const map = readModelMap(); map[provider] = model; localStorage.setItem('providerModelMap', JSON.stringify(map)); } catch { /* optional cache */ }
};

interface ModelSettingsModalProps {
  modelConfig: ModelConfig;
  setModelConfig: (config: ModelConfig | ((prev: ModelConfig) => ModelConfig)) => void;
  onSave: (config: ModelConfig) => void | Promise<void>;
  skipInitialFetch?: boolean; // Optional: skip fetching config from backend if parent manages it
  layout?: 'inline' | 'dialog';
  /**
   * Settings page mode. Renders rows for a settings card (no Save button) and
   * saves each change as it is made: selects and switches at once, text fields
   * on blur or Enter. `onSave` must throw when the save fails. Other callers
   * keep the explicit Save button.
   */
  autosave?: boolean;
}

type CatalogStatus = 'idle' | 'loading' | 'ready' | 'empty' | 'invalid' | 'forbidden' | 'unavailable';
const catalogCommands = { openai: 'get_openai_models', claude: 'get_anthropic_models', groq: 'get_groq_models' } as const;

function useHostedCatalog(provider: string, key: string | null, usable: boolean) {
  const [catalog, setCatalog] = useState<{ provider: string; models: string[]; status: CatalogStatus }>({
    provider: '', models: [], status: 'idle',
  });
  const generation = useRef(0);
  const refresh = useCallback(async () => {
    const request = ++generation.current;
    if (!(provider === 'openai' || provider === 'claude' || provider === 'groq') || !usable) {
      setCatalog({ provider, models: [], status: 'idle' });
      return;
    }
    setCatalog({ provider, models: [], status: 'loading' });
    try {
      const rows = await invoke<{ id: string }[]>(catalogCommands[provider], { apiKey: newKeyPayload(key) });
      if (generation.current === request) {
        setCatalog({ provider, models: rows.map(model => model.id), status: rows.length ? 'ready' : 'empty' });
      }
    } catch (error) {
      if (generation.current !== request) return;
      const code = error instanceof Error ? error.message : error;
      const status = code === 'invalid_api_key' ? 'invalid' : code === 'catalog_forbidden' ? 'forbidden' : 'unavailable';
      setCatalog({ provider, models: [], status });
    }
  }, [provider, key, usable]);
  useEffect(() => {
    void refresh();
    return () => { generation.current += 1; };
  }, [refresh]);
  return { catalog, refresh };
}

export function ModelSettingsModal({
  modelConfig: propsModelConfig,
  setModelConfig: propsSetModelConfig,
  onSave,
  skipInitialFetch = false,
  layout = 'inline',
  autosave = false,
}: ModelSettingsModalProps) {
  // Use ConfigContext if available, fallback to props for backward compatibility
  const configContext = useConfig();
  const sharedModelConfig = configContext?.modelConfig || propsModelConfig;
  const setSharedModelConfig = configContext?.setModelConfig || propsSetModelConfig;
  // Settings page only: a provider the user is looking at but has not switched
  // to. It is kept here and never written to the shared configuration, because
  // summaries are generated with whatever the shared configuration holds.
  const [previewConfig, setPreviewConfig] = useState<ModelConfig | null>(null);
  const previewRef = useRef<ModelConfig | null>(null);
  const setPreview = useCallback((next: ModelConfig | null) => {
    previewRef.current = next;
    setPreviewConfig(next);
  }, []);
  const modelConfig = (autosave && previewConfig) || sharedModelConfig;
  const setModelConfig = useCallback((next: ModelConfig | ((prev: ModelConfig) => ModelConfig)) => {
    const preview = previewRef.current;
    if (preview) setPreview(typeof next === 'function' ? next(preview) : next);
    else setSharedModelConfig(next);
  }, [setPreview, setSharedModelConfig]);
  const claudeCode = useClaudeCodeSettings(modelConfig.provider === 'claude-code');
  const providerKeyStatus = configContext?.providerKeyStatus;
  const refreshProviderKeyStatus = configContext?.refreshProviderKeyStatus;

  const [models, setModels] = useState<OllamaModel[]>([]);
  const [error, setError] = useState<string>('');
  // The key typed for the provider being looked at. A stored key is never loaded
  // here: the core reports only whether one is stored (`keyStatus`).
  const [draftKey, setDraftKey] = useState({ provider: modelConfig.provider, value: modelConfig.apiKey || null });
  const apiKey = draftKey.provider === modelConfig.provider ? draftKey.value : null;
  const setApiKey = (value: string | null) => setDraftKey({ provider: modelConfig.provider, value });
  const keyStatus: SecretStatus =
    providerKeyStatus?.[modelConfig.provider as KeyProvider] ?? modelConfig.apiKeyStatus ?? NO_SECRET;
  const [openRouterModels, setOpenRouterModels] = useState<OpenRouterModel[]>([]);
  const [openRouterError, setOpenRouterError] = useState<string>('');
  const [isLoadingOpenRouter, setIsLoadingOpenRouter] = useState<boolean>(false);
  const [ollamaEndpoint, setOllamaEndpoint] = useState<string>(modelConfig.ollamaEndpoint || '');
  const [isLoadingOllama, setIsLoadingOllama] = useState<boolean>(false);
  const [lastFetchedEndpoint, setLastFetchedEndpoint] = useState<string>(modelConfig.ollamaEndpoint || '');
  const [endpointValidationState, setEndpointValidationState] = useState<'valid' | 'invalid' | 'none'>('none');
  const savedOllamaEndpoint = modelConfig.ollamaEndpoint || '';
  const ollamaOnOpen = useRef({ endpoint: savedOllamaEndpoint, silent: skipInitialFetch });
  const ollamaRequest = useRef(0);
  const [searchQuery, setSearchQuery] = useState<string>('');
  const [isEndpointSectionCollapsed, setIsEndpointSectionCollapsed] = useState<boolean>(true); // Collapsed by default
  const [ollamaNotInstalled, setOllamaNotInstalled] = useState<boolean>(false); // Track if Ollama is not installed

  // Custom OpenAI state
  const [customDestinationProblem, setCustomDestinationProblem] = useState<string | null>(null);
  const [customAllowUnencrypted, setCustomAllowUnencrypted] = useState(false);
  const [customOpenAIEndpoint, setCustomOpenAIEndpoint] = useState<string>(modelConfig.customOpenAIEndpoint || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT);
  const [customOpenAIModel, setCustomOpenAIModel] = useState<string>(modelConfig.customOpenAIModel || DEFAULT_OPENAI_COMPATIBLE_MODEL);
  // A key typed for the custom provider; the stored one is only reported as a status.
  const [customOpenAIApiKey, setCustomOpenAIApiKey] = useState<string>(modelConfig.customOpenAIApiKey || '');
  const [customKeyStatus, setCustomKeyStatus] = useState<SecretStatus>(modelConfig.customOpenAIKeyStatus ?? NO_SECRET);
  // The address the stored custom key was saved for. Changing the address needs the key again.
  const [savedCustomEndpoint, setSavedCustomEndpoint] = useState<string>(modelConfig.customOpenAIEndpoint || '');
  const [customContextWindow, setCustomContextWindow] = useState<string>(modelConfig.contextWindow?.toString() || '8192');
  const [customMaxTokens, setCustomMaxTokens] = useState<string>(modelConfig.maxTokens?.toString() || '');
  const [customTemperature, setCustomTemperature] = useState<string>(modelConfig.temperature?.toString() || '');
  const [customTopP, setCustomTopP] = useState<string>(modelConfig.topP?.toString() || '');
  const [customTimeoutSeconds, setCustomTimeoutSeconds] = useState<string>(modelConfig.timeoutSeconds?.toString() || DEFAULT_OPENAI_COMPATIBLE_TIMEOUT_SECONDS);
  const [customOrganization, setCustomOrganization] = useState<string>(modelConfig.organization || '');
  const [customProject, setCustomProject] = useState<string>(modelConfig.project || '');
  const [isCustomOpenAIAdvancedOpen, setIsCustomOpenAIAdvancedOpen] = useState<boolean>(false);
  const [isTestingConnection, setIsTestingConnection] = useState<boolean>(false);

  // Combobox state
  const [modelComboboxOpen, setModelComboboxOpen] = useState<boolean>(false);

  const { catalog, refresh: refreshHostedCatalog } = useHostedCatalog(
    modelConfig.provider, apiKey, hasUsableKey(apiKey, keyStatus),
  );
  const [openAIAuthStatus, setOpenAIAuthStatus] = useState<OpenAIAuthStatus | null>(null);
  const [isLoadingOpenAIAuthStatus, setIsLoadingOpenAIAuthStatus] = useState<boolean>(false);
  const [codexConfig, setCodexConfig] = useState<CodexProviderConfig>({
    codexHomeMode: 'clawscribe-isolated',
    codexHomePath: null,
    useExistingUserCodexSession: false,
    codexBinaryPath: null,
    model: CODEX_DEFAULT_MODEL,
    timeoutSeconds: 600,
  });
  const [codexStatus, setCodexStatus] = useState<CodexInstallationStatus | null>(null);
  const [codexLastResult, setCodexLastResult] = useState<string>('');
  const [isCodexBusy, setIsCodexBusy] = useState<boolean>(false);
  // Only the live app-server catalog is offered; a stale built-in list would hide new models.
  const [codexModels, setCodexModels] = useState<CodexModelInfo[]>([]);
  const [codexModelError, setCodexModelError] = useState<string>('');
  const [isLoadingCodexModels, setIsLoadingCodexModels] = useState<boolean>(false);

  // Use global download context instead of local state
  const { isDownloading, getProgress, downloadingModels } = useOllamaDownload();

  // Built-in AI models state
  const [builtinAiModels, setBuiltinAiModels] = useState<BuiltInModelInfo[]>([]);

  // Settings page (autosave) state
  const feedback = useSaveFeedback();
  const [advancedOpen, setAdvancedOpen] = useState<boolean>(false);
  const [customCheck, setCustomCheck] = useState<{ state: 'idle' | 'checking' | 'ok' | 'error'; message: string }>({ state: 'idle', message: '' });
  const [claudeRecheck, setClaudeRecheck] = useState<number>(0);
  const [claudeGuideError, setClaudeGuideError] = useState<string>('');

  // Cache models by endpoint to avoid refetching when reverting endpoint changes
  const modelsCache = useRef<Map<string, OllamaModel[]>>(new Map());

  // URL validation helper
  const validateOllamaEndpoint = (url: string): boolean => {
    if (!url.trim()) return true; // Empty is valid (uses default)
    try {
      const parsed = new URL(url);
      return parsed.protocol === 'http:' || parsed.protocol === 'https:';
    } catch {
      return false;
    }
  };

  // Debounced URL validation with visual feedback
  useEffect(() => {
    const timer = setTimeout(() => {
      const trimmed = ollamaEndpoint.trim();

      if (!trimmed) {
        setEndpointValidationState('none');
      } else if (validateOllamaEndpoint(trimmed)) {
        setEndpointValidationState('valid');
      } else {
        setEndpointValidationState('invalid');
      }
    }, 500); // 500ms debounce

    return () => clearTimeout(timer);
  }, [ollamaEndpoint]);

  const configuredCodexModel = codexConfig.model || modelConfig.model || CODEX_DEFAULT_MODEL;
  const codexPickerModels = useMemo(() => codexModels.some((model) => model.id === configuredCodexModel)
    ? codexModels
    : [
        {
          id: configuredCodexModel,
          displayName: configuredCodexModel,
          hidden: false,
          isDefault: false,
          defaultReasoningEffort: null,
          supportedReasoningEfforts: [],
          inputModalities: ['text', 'image'],
        },
        ...codexModels,
      ], [codexModels, configuredCodexModel]);

  const modelOptions = useMemo<Record<string, string[]>>(() => ({
    ollama: models.map((model) => model.name),
    claude: catalog.provider === 'claude' && catalog.models.length ? catalog.models : CLAUDE_FALLBACK_MODELS,
    groq: catalog.provider === 'groq' && catalog.models.length ? catalog.models : GROQ_FALLBACK_MODELS,
    openai: catalog.provider === 'openai' && catalog.models.length ? catalog.models : OPENAI_FALLBACK_MODELS,
    openrouter: openRouterModels.map((m) => m.id),
    'builtin-ai': builtinAiModels.map((m) => m.name),
    'custom-openai': [customOpenAIModel || DEFAULT_OPENAI_COMPATIBLE_MODEL],
    codex: codexPickerModels.map((model) => model.id),
    'claude-code': CLAUDE_CODE_MODELS,
  }), [models, catalog, openRouterModels, builtinAiModels, customOpenAIModel, codexPickerModels]);

  const requiresApiKey =
    modelConfig.provider === 'claude' ||
    modelConfig.provider === 'groq' ||
    modelConfig.provider === 'openai' ||
    modelConfig.provider === 'openrouter';

  // Check if Ollama endpoint has changed but models haven't been fetched yet
  const ollamaEndpointChanged = modelConfig.provider === 'ollama' &&
    ollamaEndpoint.trim() !== lastFetchedEndpoint.trim();

  // Custom OpenAI validation
  const isCustomOpenAIInvalid = modelConfig.provider === 'custom-openai' && (
    !customOpenAIEndpoint.trim() ||
    !customOpenAIModel.trim() ||
    !customTimeoutSeconds.trim()
  );

  // A stored key goes only to the address it was saved for: changing the
  // address asks for the key again, and nothing is saved until it is entered.
  const customKeyRequired = keyRequiredForAddress(savedCustomEndpoint, customOpenAIEndpoint.trim() || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT, customKeyStatus, customOpenAIApiKey);
  const typedKeyProblem = keyProblem(apiKey) || keyProblem(customOpenAIApiKey);

  const isDoneDisabled =
    (modelConfig.provider === 'claude-code' && claudeCode.busy) ||
    (requiresApiKey && !hasUsableKey(apiKey, keyStatus)) ||
    (modelConfig.provider === 'ollama' && ollamaEndpointChanged) ||
    isCustomOpenAIInvalid ||
    (modelConfig.provider === 'custom-openai' && customKeyRequired) ||
    typedKeyProblem !== null;

  useEffect(() => {
    const fetchModelConfig = async () => {
      if (skipInitialFetch) return;

      try {
        const data = await invoke<ModelConfig | null>('api_get_model_config');
        if (data && data.provider !== null) {
          setModelConfig({ ...withoutSecrets(data), apiKeyStatus: readSecretStatus(data.apiKeyStatus) });

          // Sync ollamaEndpoint state with fetched config
          if (data.ollamaEndpoint) {
            setOllamaEndpoint(data.ollamaEndpoint);
            // Don't set lastFetchedEndpoint here - it will be set after successful model fetch
          }

          // Fetch Custom OpenAI config if that's the active provider
          if (data.provider === 'custom-openai') {
            try {
              const customConfig = await invoke<CustomOpenAIConfig | null>('api_get_custom_openai_config');
              if (customConfig) {
                setCustomOpenAIEndpoint(customConfig.endpoint || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT);
                setCustomOpenAIModel(customConfig.model || DEFAULT_OPENAI_COMPATIBLE_MODEL);
                setCustomAllowUnencrypted(customConfig.allow_unencrypted ?? false);
                setCustomDestinationProblem(customConfig.destination_problem ?? null);
                setCustomKeyStatus(readSecretStatus(customConfig.apiKeyStatus));
                setSavedCustomEndpoint(customConfig.endpoint || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT);
                setCustomMaxTokens(customConfig.maxTokens?.toString() || '');
                setCustomContextWindow(customConfig.contextWindow?.toString() || '8192');
                setCustomTemperature(customConfig.temperature?.toString() || '');
                setCustomTopP(customConfig.topP?.toString() || '');
                setCustomTimeoutSeconds(customConfig.timeoutSeconds?.toString() || DEFAULT_OPENAI_COMPATIBLE_TIMEOUT_SECONDS);
                setCustomOrganization(customConfig.organization || '');
                setCustomProject(customConfig.project || '');
              }
            } catch (err) {
              console.error('Failed to fetch custom OpenAI config:', err);
            }
          }
        }
      } catch (error) {
        console.error('Failed to fetch model config:', error);
      }
    };

    fetchModelConfig();
  }, [skipInitialFetch, setModelConfig]);

  // Sync ollamaEndpoint state when modelConfig.ollamaEndpoint changes from parent
  useEffect(() => {
    setOllamaEndpoint(modelConfig.ollamaEndpoint || '');
  }, [modelConfig.ollamaEndpoint, modelConfig.provider]);

  // Sync custom OpenAI state from modelConfig (context or props)
  useEffect(() => {
    if (modelConfig.provider === 'custom-openai') {
      console.log('Syncing custom OpenAI fields from ConfigContext:', {
        endpoint: modelConfig.customOpenAIEndpoint,
        model: modelConfig.customOpenAIModel,
        hasStoredKey: !!modelConfig.customOpenAIKeyStatus?.stored,
      });

      // Always sync from modelConfig (which comes from context if available)
      setCustomOpenAIEndpoint(modelConfig.customOpenAIEndpoint || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT);
      setCustomOpenAIModel(modelConfig.customOpenAIModel || DEFAULT_OPENAI_COMPATIBLE_MODEL);
      if (modelConfig.customOpenAIKeyStatus) setCustomKeyStatus(modelConfig.customOpenAIKeyStatus);
      setCustomMaxTokens(modelConfig.maxTokens?.toString() || '');
      setCustomContextWindow(modelConfig.contextWindow?.toString() || '8192');
      setCustomTemperature(modelConfig.temperature?.toString() || '');
      setCustomTopP(modelConfig.topP?.toString() || '');
      setCustomTimeoutSeconds(modelConfig.timeoutSeconds?.toString() || DEFAULT_OPENAI_COMPATIBLE_TIMEOUT_SECONDS);
      setCustomOrganization(modelConfig.organization || '');
      setCustomProject(modelConfig.project || '');
    }
  }, [
    modelConfig.provider,
    modelConfig.customOpenAIEndpoint,
    modelConfig.customOpenAIModel,
    modelConfig.customOpenAIKeyStatus,
    modelConfig.maxTokens,
    modelConfig.contextWindow,
    modelConfig.temperature,
    modelConfig.topP,
    modelConfig.timeoutSeconds,
    modelConfig.organization,
    modelConfig.project
  ]);

  // Clear provider-specific state when leaving Ollama
  useEffect(() => {
    if (modelConfig.provider !== 'ollama') {
      setModels([]); // Clear models list
      setError(''); // Clear any error state
      setOllamaNotInstalled(false); // Reset installation status
    }
  }, [modelConfig.provider]);

  // Handle endpoint changes - restore cached models or clear
  useEffect(() => {
    if (modelConfig.provider === 'ollama' &&
      ollamaEndpoint.trim() !== lastFetchedEndpoint.trim()) {

      // Check if we have cached models for this endpoint (including empty endpoint = default)
      const cachedModels = modelsCache.current.get(ollamaEndpoint.trim());

      if (cachedModels && cachedModels.length > 0) {
        // Restore cached models and update tracking
        setModels(cachedModels);
        setLastFetchedEndpoint(ollamaEndpoint.trim());
        setError('');
      } else {
        // No cache - clear models and allow refetch
        setModels([]);
        setError('');
      }
    }
  }, [ollamaEndpoint, lastFetchedEndpoint, modelConfig.provider]);

  // A key typed for one provider is never carried to another.
  useEffect(() => {
    setDraftKey(prev => prev.provider === modelConfig.provider ? prev : { provider: modelConfig.provider, value: null });
  }, [modelConfig.provider]);

  useEffect(() => {
    setAdvancedOpen(false);
    setCustomCheck({ state: 'idle', message: '' });
    setClaudeGuideError('');
  }, [modelConfig.provider]);

  // Re-check Claude Code after the program path was saved, once the new path is in state.
  const lastClaudeRecheck = useRef(0);
  const { check: checkClaudeCode } = claudeCode;
  useEffect(() => {
    if (claudeRecheck === lastClaudeRecheck.current) return;
    lastClaudeRecheck.current = claudeRecheck;
    void checkClaudeCode();
  }, [claudeRecheck, checkClaudeCode]);

  // Manual fetch function for Ollama models
  const fetchOllamaModels = useCallback(async (silent: boolean, rawEndpoint: string) => {
    const request = ++ollamaRequest.current;
    const trimmedEndpoint = rawEndpoint.trim();

    // Validate URL if provided
    if (trimmedEndpoint && !validateOllamaEndpoint(trimmedEndpoint)) {
      const errorMsg = 'Invalid Ollama endpoint URL. Must start with http:// or https://';
      setError(errorMsg);
      setIsLoadingOllama(false);
      if (!silent) {
        toast.error(errorMsg);
      }
      return;
    }

    setIsLoadingOllama(true);
    setError(''); // Clear previous errors

    try {
      const endpoint = trimmedEndpoint || null;
      const modelList = (await invoke('get_ollama_models', { endpoint })) as OllamaModel[];
      if (request !== ollamaRequest.current) return;
      setModels(modelList);
      setLastFetchedEndpoint(trimmedEndpoint); // Track successful fetch

      // Cache the fetched models for this endpoint
      modelsCache.current.set(trimmedEndpoint, modelList);

      // Successfully fetched models, Ollama is installed
      setOllamaNotInstalled(false);
    } catch (err) {
      if (request !== ollamaRequest.current) return;
      const errorMsg = err instanceof Error ? err.message : 'Failed to load Ollama models';
      setError(errorMsg);

      // Check if error indicates Ollama is not installed
      if (isOllamaNotInstalledError(errorMsg)) {
        setOllamaNotInstalled(true);
      } else {
        setOllamaNotInstalled(false);
      }

      if (!silent) {
        toast.error(errorMsg);
      }
      console.error('Error loading models:', err);
    } finally {
      if (request === ollamaRequest.current) setIsLoadingOllama(false);
    }
  }, []);

  useEffect(() => {
    ollamaOnOpen.current = { endpoint: savedOllamaEndpoint, silent: skipInitialFetch };
  }, [savedOllamaEndpoint, skipInitialFetch]);

  // Opening Ollama checks the saved endpoint. Editing its URL is local until
  // the user requests a fetch or commits the field.
  useEffect(() => {
    if (modelConfig.provider !== 'ollama') return;
    const { silent, endpoint } = ollamaOnOpen.current;
    void fetchOllamaModels(silent, endpoint);
    return () => { ollamaRequest.current += 1; };
  }, [modelConfig.provider, fetchOllamaModels]);

  const loadOpenRouterModels = async () => {
    if (openRouterModels.length > 0) return; // Already loaded

    try {
      setIsLoadingOpenRouter(true);
      setOpenRouterError('');
      const data = (await invoke('get_openrouter_models')) as OpenRouterModel[];
      setOpenRouterModels(data);
    } catch (err) {
      console.error('Error loading OpenRouter models:', err);
      setOpenRouterError(
        err instanceof Error ? err.message : 'Failed to load OpenRouter models'
      );
    } finally {
      setIsLoadingOpenRouter(false);
    }
  };

  const loadBuiltinAiModels = async () => {
    if (builtinAiModels.length > 0) return; // Already loaded

    try {
      const data = await invoke<BuiltInModelInfo[]>('builtin_ai_list_models');
      setBuiltinAiModels(data);
    } catch (err) {
      console.error('Error loading Built-in AI models:', err);
      toast.error('Failed to load Built-in AI models');
    }
  };

  const loadOpenAIAuthStatus = async () => {
    setIsLoadingOpenAIAuthStatus(true);
    try {
      const status = (await invoke('api_get_openai_auth_status')) as OpenAIAuthStatus;
      setOpenAIAuthStatus(status);
    } catch (err) {
      console.error('Error loading OpenAI auth status:', err);
      setOpenAIAuthStatus(null);
    } finally {
      setIsLoadingOpenAIAuthStatus(false);
    }
  };

  const loadCodexModels = useCallback(async () => {
    setIsLoadingCodexModels(true);
    setCodexModelError('');
    try {
      const models = (await invoke('codex_list_models')) as CodexModelInfo[];
      if (models.length === 0) {
        throw new Error('Codex app-server returned no picker-visible models');
      }
      setCodexModels(models);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      console.error('Failed to load Codex model catalog:', err);
      setCodexModels([]);
      setCodexModelError(message);
    } finally {
      setIsLoadingCodexModels(false);
    }
  }, []);

  const saveCodexConfig = useCallback(async (nextConfig: CodexProviderConfig) => {
    const normalized = {
      ...nextConfig,
      codexHomeMode: 'clawscribe-isolated' as const,
      useExistingUserCodexSession: false,
      codexBinaryPath: null,
      model: nextConfig.model.trim() || CODEX_DEFAULT_MODEL,
      timeoutSeconds: nextConfig.timeoutSeconds || 600,
    };
    const saved = (await invoke('codex_save_config', { config: normalized })) as { processing?: { codex?: CodexProviderConfig } };
    if (saved.processing?.codex) {
      setCodexConfig(saved.processing.codex);
    }
    return normalized;
  }, []);

  const checkCodexInstallation = useCallback(async (nextConfig?: CodexProviderConfig) => {
    setIsCodexBusy(true);
    try {
      if (nextConfig) await saveCodexConfig(nextConfig);
      const status = (await invoke('codex_check_installation')) as CodexInstallationStatus;
      setCodexStatus(status);
      setCodexLastResult(status.message);
      if (status.found) {
        await loadCodexModels();
        toast.success(`Codex app-server runtime found: ${status.version || status.path || 'installed'}`);
      } else {
        toast.error(status.message || 'Bundled Codex runtime is missing or damaged');
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setCodexLastResult(message);
      toast.error(message);
    } finally {
      setIsCodexBusy(false);
    }
  }, [loadCodexModels, saveCodexConfig]);

  const findCodexAutomatically = async () => {
    setIsCodexBusy(true);
    try {
      await saveCodexConfig({ ...codexConfig, codexBinaryPath: null });
      const status = (await invoke('codex_find_automatically')) as CodexInstallationStatus;
      setCodexStatus(status);
      setCodexLastResult(status.message);
      if (status.found && status.path) {
        setCodexConfig((prev) => ({ ...prev, codexBinaryPath: null }));
        await loadCodexModels();
        toast.success(`Codex app-server runtime found: ${status.version || status.path}`);
      } else {
        toast.error(status.message || 'Bundled Codex runtime is missing or damaged');
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setCodexLastResult(message);
      toast.error(message);
    } finally {
      setIsCodexBusy(false);
    }
  };

  const prepareCodexInstallRepair = async () => {
    setIsCodexBusy(true);
    try {
      const plan = (await invoke('codex_prepare_install_command')) as CodexInstallRepairPlan;
      const alternatives = plan.alternatives
        .map((item) => `${item.label} (${item.shell}):\n${item.command}`)
        .join('\n\n');
      setCodexLastResult([
        plan.message,
        '',
        `${plan.recommended.label} (${plan.recommended.shell}):`,
        plan.recommended.command,
        alternatives ? `\nAlternatives:\n${alternatives}` : '',
        `\nDocs: ${plan.docsUrl}`,
      ].filter(Boolean).join('\n'));
      toast.info('Codex app-server repair information prepared.');
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setCodexLastResult(message);
      toast.error(message);
    } finally {
      setIsCodexBusy(false);
    }
  };

  const runCodexAction = async (command: 'codex_login_browser' | 'codex_login_device' | 'codex_logout' | 'codex_test_app_server' | 'codex_test_processing') => {
    setIsCodexBusy(true);
    try {
      await saveCodexConfig(codexConfig);
      const result = (await invoke(command)) as CodexCommandStatus;
      const detail = [result.message, result.stdout, result.stderr].filter(Boolean).join('\n').trim();
      setCodexLastResult(detail || (result.success ? 'Codex command completed' : 'Codex command failed'));
      if (result.success) {
        toast.success(result.message || 'Codex command completed');
      } else {
        toast.error(result.message || 'Codex command failed');
      }
      await checkCodexInstallation();
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setCodexLastResult(message);
      toast.error(message);
    } finally {
      setIsCodexBusy(false);
    }
  };

  useEffect(() => {
    if (modelConfig.provider === 'openai') {
      loadOpenAIAuthStatus();
    }
  }, [modelConfig.provider]);

  useEffect(() => {
    if (modelConfig.provider !== 'codex') return;
    let active = true;
    const load = async () => {
      let codex: CodexProviderConfig | undefined;
      try {
        const config = await invoke<{ processing?: { codex?: CodexProviderConfig } }>('codex_get_config');
        if (!active) return;
        codex = config.processing?.codex;
        if (codex) {
          setCodexConfig(codex);
          const model = codex.model;
          if (model) setModelConfig(prev => prev.provider === 'codex' && prev.model !== model ? { ...prev, model } : prev);
        }
      } catch (err) {
        if (!active) return;
        console.error('Failed to load Codex config:', err);
        setCodexLastResult(err instanceof Error ? err.message : String(err));
      }
      await checkCodexInstallation(codex);
    };
    void load();
    return () => { active = false; };
  }, [modelConfig.provider, setModelConfig, checkCodexInstallation]);

  // Auto-fetch Claude models when provider is claude and we have an API key
  // Restore cached model when async model lists become available
  useEffect(() => {
    const providerModels = modelOptions[modelConfig.provider];
    if (!providerModels || providerModels.length === 0) return;

    // If current model is already valid, nothing to do
    if (modelConfig.model && providerModels.includes(modelConfig.model)) return;

    // Try to restore from localStorage cache
    if (modelConfig.provider === 'builtin-ai' && !modelConfig.model) return;
    const cachedModel = readModelMap()[modelConfig.provider];
    if (cachedModel && providerModels.includes(cachedModel)) {
      setModelConfig((prev: ModelConfig) => ({ ...prev, model: cachedModel }));
    }
  }, [modelOptions, modelConfig.provider, modelConfig.model, setModelConfig]);

  const handleSave = async () => {
    const problem = keyProblem(apiKey) || keyProblem(customOpenAIApiKey);
    if (problem) { toast.error(problem); return; }
    if (modelConfig.provider === 'custom-openai' && customKeyRequired) { toast.error(KEY_REQUIRED_FOR_NEW_ADDRESS); return; }
    if (modelConfig.provider === 'claude-code') {
      try { await claudeCode.save(); }
      catch { toast.error('Could not save Claude Code settings'); return; }
    }
    // For custom-openai provider, save the custom config first
    if (modelConfig.provider === 'custom-openai') {
      try {
        await invoke('api_save_custom_openai_config', {
        allowUnencrypted: customAllowUnencrypted,
          endpoint: customOpenAIEndpoint.trim() || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT,
          apiKey: newKeyPayload(customOpenAIApiKey),
          model: customOpenAIModel.trim() || DEFAULT_OPENAI_COMPATIBLE_MODEL,
          maxTokens: customMaxTokens ? parseInt(customMaxTokens, 10) : null,
          contextWindow: Number(customContextWindow) || 8192,
          temperature: customTemperature ? parseFloat(customTemperature) : null,
          topP: customTopP ? parseFloat(customTopP) : null,
          timeoutSeconds: customTimeoutSeconds ? parseInt(customTimeoutSeconds, 10) : null,
          organization: customOrganization.trim() || null,
          project: customProject.trim() || null,
        });
        console.log('Custom OpenAI config saved successfully');
      } catch (err) {
        console.error('Failed to save custom OpenAI config:', err);
        toast.error('Failed to save custom OpenAI configuration');
        return;
      }
    }

    const updatedConfig = {
      ...modelConfig,
      // Only a key typed now is sent; without one the stored key is kept.
      apiKey: requiresApiKey ? newKeyPayload(apiKey) : null,
      ollamaEndpoint: modelConfig.provider === 'ollama'
        ? (ollamaEndpoint.trim() || null)
        : (modelConfig.ollamaEndpoint || null),
      // Include custom OpenAI fields
      customOpenAIEndpoint: modelConfig.provider === 'custom-openai' ? (customOpenAIEndpoint.trim() || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT) : null,
      customOpenAIModel: modelConfig.provider === 'custom-openai' ? (customOpenAIModel.trim() || DEFAULT_OPENAI_COMPATIBLE_MODEL) : null,
      customOpenAIApiKey: modelConfig.provider === 'custom-openai' ? newKeyPayload(customOpenAIApiKey) : null,
      contextWindow: modelConfig.provider === 'custom-openai' ? Number(customContextWindow) || 8192 : null,
      maxTokens: modelConfig.provider === 'custom-openai' && customMaxTokens ? parseInt(customMaxTokens, 10) : null,
      temperature: modelConfig.provider === 'custom-openai' && customTemperature ? parseFloat(customTemperature) : null,
      topP: modelConfig.provider === 'custom-openai' && customTopP ? parseFloat(customTopP) : null,
      timeoutSeconds: modelConfig.provider === 'custom-openai' && customTimeoutSeconds ? parseInt(customTimeoutSeconds, 10) : null,
      organization: modelConfig.provider === 'custom-openai' && customOrganization.trim() ? customOrganization.trim() : null,
      project: modelConfig.provider === 'custom-openai' && customProject.trim() ? customProject.trim() : null,
      // For custom-openai, use the customOpenAIModel as the model field
      model: modelConfig.provider === 'custom-openai' ? (customOpenAIModel.trim() || DEFAULT_OPENAI_COMPATIBLE_MODEL) : modelConfig.model,
    };
    setModelConfig(updatedConfig);

    // Persist confirmed model choice to per-provider cache
    if (updatedConfig.model) {
      const map = JSON.parse(localStorage.getItem('providerModelMap') || '{}');
      map[updatedConfig.provider] = updatedConfig.model;
      localStorage.setItem('providerModelMap', JSON.stringify(map));
    }

    if (updatedConfig.provider === 'openai') {
      try {
        const status = (await invoke('api_save_openai_auth_config', {
          config: { mode: 'api_key' },
        })) as OpenAIAuthStatus;
        setOpenAIAuthStatus(status);
      } catch (err) {
        console.error('Failed to save OpenAI auth mode:', err);
        toast.error('Failed to save OpenAI auth mode');
        return;
      }
    }

    if (updatedConfig.provider === 'codex') {
      try {
        const savedCodexConfig = await saveCodexConfig({
          ...codexConfig,
          model: codexConfig.model.trim() || updatedConfig.model || CODEX_DEFAULT_MODEL,
        });
        updatedConfig.model = savedCodexConfig.model;
        setModelConfig(updatedConfig);
      } catch (err) {
        const errorMsg = err instanceof Error ? err.message : String(err);
        console.error('Failed to save Codex configuration:', err);
        toast.error(errorMsg || 'Failed to save Codex configuration');
        return;
      }
    }

    await Promise.resolve(onSave(updatedConfig));

    // The typed key is stored now: drop it and ask the core what is stored.
    await afterKeySaved(updatedConfig.provider);

    if (updatedConfig.provider === 'openai') {
      await loadOpenAIAuthStatus();
    }
  };

  /** Forget a typed key once it is stored, and show what the core reports instead. */
  const afterKeySaved = async (provider: string) => {
    setApiKey(null);
    setCustomOpenAIApiKey('');
    setModelConfig((prev: ModelConfig) => ({ ...prev, apiKey: null, customOpenAIApiKey: null }));
    await refreshProviderKeyStatus?.(provider);
    if (provider === 'custom-openai') await refreshCustomKeyStatus();
  };

  const refreshCustomKeyStatus = async () => {
    try {
      const config = await configService.getCustomOpenAIConfig();
      setCustomKeyStatus(config?.apiKeyStatus ?? NO_SECRET);
      if (config?.endpoint) setSavedCustomEndpoint(config.endpoint);
    } catch {
      console.error('Failed to read the custom provider key status');
    }
  };

  /** What removing a provider's key breaks, when that provider is the one in use. */
  const removeWarning = (provider: string) =>
    removalNeedsConfirmation(provider, sharedModelConfig.provider)
      ? 'Summaries with this provider will not work until you add a key again.'
      : null;

  const removeStoredKey = async () => {
    const provider = modelConfig.provider;
    await configService.clearApiKey(provider);
    setApiKey(null);
    setModelConfig((prev: ModelConfig) => ({ ...prev, apiKey: null }));
    await refreshProviderKeyStatus?.(provider);
  };

  const removeCustomKey = async () => {
    await configService.clearCustomOpenAIApiKey();
    setCustomOpenAIApiKey('');
    await refreshCustomKeyStatus();
    await refreshProviderKeyStatus?.('custom-openai');
  };

  const customOpenAIInvokeConfig = () => ({
    endpoint: customOpenAIEndpoint.trim() || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT,
    apiKey: newKeyPayload(customOpenAIApiKey),
    model: customOpenAIModel.trim() || DEFAULT_OPENAI_COMPATIBLE_MODEL,
    maxTokens: customMaxTokens ? parseInt(customMaxTokens, 10) : null,
          contextWindow: Number(customContextWindow) || 8192,
    temperature: customTemperature ? parseFloat(customTemperature) : null,
    topP: customTopP ? parseFloat(customTopP) : null,
    timeoutSeconds: customTimeoutSeconds ? parseInt(customTimeoutSeconds, 10) : null,
    organization: customOrganization.trim() || null,
    project: customProject.trim() || null,
  });

  // Test custom OpenAI connection
  const testCustomOpenAIConnection = async () => {
    if (!customOpenAIEndpoint.trim() || !customOpenAIModel.trim()) {
      toast.error('Please enter endpoint URL and model name first');
      return;
    }

    setIsTestingConnection(true);
    try {
      const result = await invoke<{ status: string; message: string }>('api_test_custom_openai_connection', {
        allowUnencrypted: customAllowUnencrypted,
        ...customOpenAIInvokeConfig(),
      });
      toast.success(result.message || 'Connection successful');
    } catch (err) {
      const errorMsg = err instanceof Error ? err.message : String(err);
      toast.error(errorMsg);
    } finally {
      setIsTestingConnection(false);
    }
  };

  const testCustomOpenAIProcessing = async () => {
    if (!customOpenAIEndpoint.trim() || !customOpenAIModel.trim()) {
      toast.error('Please enter endpoint URL and model name first');
      return;
    }

    setIsTestingConnection(true);
    try {
      const result = await invoke<{ status: string; message: string; outputJsonPath?: string }>('api_test_custom_openai_processing', {
        allowUnencrypted: customAllowUnencrypted,
        ...customOpenAIInvokeConfig(),
      });
      toast.success(result.message || 'Test meeting processed successfully');
    } catch (err) {
      const errorMsg = err instanceof Error ? err.message : String(err);
      toast.error(errorMsg);
    } finally {
      setIsTestingConnection(false);
    }
  };

  const clearCustomOpenAICredentials = async () => {
    try {
      await removeCustomKey();
      setCustomOrganization('');
      setCustomProject('');
      toast.success('The saved key was removed. Click Save to clear the organization and project.');
    } catch {
      toast.error('Could not remove the saved key');
    }
  };

  // Function to download recommended model
  const downloadRecommendedModel = async () => {
    const recommendedModel = 'gemma3:1b';

    // Prevent duplicate downloads (defense in depth - backend also checks)
    if (isDownloading(recommendedModel)) {
      toast.info(`${recommendedModel} is already downloading`, {
        description: `Progress: ${Math.round(getProgress(recommendedModel) || 0)}%`
      });
      return;
    }

    try {
      const endpoint = ollamaEndpoint.trim() || null;

      // The download will be tracked by the global context via events
      // Progress toasts are shown automatically by OllamaDownloadContext
      await invoke('pull_ollama_model', {
        modelName: recommendedModel,
        endpoint
      });

      // Refresh the models list after successful download
      await fetchOllamaModels(true, ollamaEndpoint);

      // Note: Model is NOT auto-selected - user must explicitly choose it
      // This respects the database as the single source of truth
    } catch (err) {
      const errorMsg = err instanceof Error ? err.message : 'Failed to download model';
      console.error('Error downloading model:', err);

      // Check if Ollama is not installed and show appropriate error
      if (isOllamaNotInstalledError(errorMsg)) {
        toast.error('Ollama is not installed', {
          description: 'Please download and install Ollama before downloading models.',
          duration: 7000,
          action: {
            label: 'Download',
            onClick: () => openExternal('https://ollama.com/download')
          }
        });
        // Update the installation status flag
        setOllamaNotInstalled(true);
      }
      // Other errors are handled by the context
    }
  };

  // Track previous downloading models to detect completions
  const previousDownloadingRef = useRef<Set<string>>(new Set());

  // Refresh models list when download completes
  useEffect(() => {
    const current = downloadingModels;
    const previous = previousDownloadingRef.current;

    // Check if any downloads completed (were in previous, not in current)
    for (const modelName of previous) {
      if (modelConfig.provider === 'ollama' && !current.has(modelName)) {
        // Download completed, refresh models list
        console.log(`[ModelSettingsModal] Download completed for ${modelName}, refreshing list`);
        fetchOllamaModels(true, ollamaEndpoint);
        break; // Only refresh once even if multiple completed
      }
    }

    // Update ref for next comparison
    previousDownloadingRef.current = new Set(current);
  }, [downloadingModels, fetchOllamaModels, ollamaEndpoint, modelConfig.provider]);

  // Filter Ollama models based on search query
  const filteredModels = models.filter((model) => {
    if (!searchQuery.trim()) return true;

    const query = searchQuery.toLowerCase();
    const isLoaded = modelConfig.model === model.name;
    const loadedText = isLoaded ? 'loaded' : '';

    return (
      model.name.toLowerCase().includes(query) ||
      model.size.toLowerCase().includes(query) ||
      loadedText.includes(query)
    );
  });

  // ---------------------------------------------------------------------------
  // Settings page mode: rows for a settings card, every change saved as made.
  // ---------------------------------------------------------------------------
  // While another provider is only being looked at, changes stay with the
  // preview and resolve as "held". Only `switching` (the "Use" button) saves.
  const saveConfig = async (next: ModelConfig, failure: string, switching = false): Promise<CommitResult> => {
    if (previewRef.current && !switching) {
      setPreview(next);
      return 'held';
    }
    try { await Promise.resolve(onSave(next)); } catch { throw new SaveError(failure); }
    setPreview(null);
  };

  // Looking at another provider must not replace a working setup. The select
  // only changes what is shown; the saved provider stays in use until the user
  // switches with "Use ..." or saves a setting of the new provider.
  const changeProviderAutosave = (value: string) => {
    const provider = value as ModelConfig['provider'];
    setError('');
    if (provider === sharedModelConfig.provider) {
      setPreview(null);
      return;
    }
    if (provider === modelConfig.provider) return;
    {
      if (!previewRef.current && sharedModelConfig.model) rememberModel(sharedModelConfig.provider, sharedModelConfig.model);
      const saved = readModelMap()[provider];
      const options = modelOptions[provider];
      const model = saved && options?.includes(saved) ? saved : (options?.[0] ?? '');
      const next: ModelConfig = {
        ...sharedModelConfig,
        provider,
        model,
        apiKey: null,
        apiKeyStatus: needsKey(provider) ? providerKeyStatus?.[provider as KeyProvider] : undefined,
      };
      setPreview(next);
      if (provider === 'openrouter') void loadOpenRouterModels();
      if (provider === 'builtin-ai') void loadBuiltinAiModels();
      if (provider === 'custom-openai') {
        invoke<CustomOpenAIConfig | null>('api_get_custom_openai_config').then((config) => {
          if (!config) return;
          setCustomOpenAIEndpoint(config.endpoint || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT);
          setCustomOpenAIModel(config.model || DEFAULT_OPENAI_COMPATIBLE_MODEL);
          setCustomAllowUnencrypted(config.allow_unencrypted ?? false);
          setCustomDestinationProblem(config.destination_problem ?? null);
          setCustomKeyStatus(readSecretStatus(config.apiKeyStatus));
          setSavedCustomEndpoint(config.endpoint || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT);
          setCustomMaxTokens(config.maxTokens?.toString() || '');
          setCustomContextWindow(config.contextWindow?.toString() || '8192');
          setCustomTemperature(config.temperature?.toString() || '');
          setCustomTopP(config.topP?.toString() || '');
          setCustomTimeoutSeconds(config.timeoutSeconds?.toString() || DEFAULT_OPENAI_COMPATIBLE_TIMEOUT_SECONDS);
          setCustomOrganization(config.organization || '');
          setCustomProject(config.project || '');
        }).catch(() => { /* the fields keep their last values */ });
      }
    }
  };

  /** What still has to be chosen before the provider being looked at can be used. */
  const draftProviderMissing = (): string | null => {
    if (needsKey(modelConfig.provider) && !hasUsableKey(modelConfig.apiKey, keyStatus)) return 'Enter an API key first';
    if (MODEL_LIST_PROVIDERS.includes(modelConfig.provider) && !modelConfig.model?.trim()) return 'Choose a model first';
    if (modelConfig.provider === 'codex' && codexStatus && !codexStatus.found) return 'Install the Codex runtime first';
    if (modelConfig.provider === 'claude-code' && claudeCode.status && claudeCode.status.state !== 'ready') return 'Claude Code is not ready yet';
    return null;
  };

  const useDraftProvider = () => {
    void feedback.run('provider', async () => {
      // Only the key stored for this provider is used, never one typed for another provider.
      const next: ModelConfig = {
        ...modelConfig,
        whisperModel: sharedModelConfig.whisperModel,
        ollamaEndpoint: modelConfig.provider === 'ollama' ? modelConfig.ollamaEndpoint : sharedModelConfig.ollamaEndpoint,
        apiKey: needsKey(modelConfig.provider) ? newKeyPayload(modelConfig.apiKey) : null,
      };
      await saveConfig(next, 'Could not save the provider', true);
      setError('');
      setModelConfig({ ...next, apiKey: null });
      setApiKey(null);
      await refreshProviderKeyStatus?.(next.provider);
    });
  };

  const chooseModelAutosave = (model: string) => {
    void feedback.run('model', async () => {
      const next: ModelConfig = { ...modelConfig, model, apiKey: needsKey(modelConfig.provider) ? newKeyPayload(modelConfig.apiKey) : null };
      const held = await saveConfig(next, 'Could not save the model');
      rememberModel(modelConfig.provider, model);
      setModelConfig(next);
      return held;
    });
  };

  // The core opens the folder dialog and only accepts the folder it chose here
  // (new or empty, or already ClawScribe's own Codex profile).
  const chooseCodexHome = async () => {
    try {
      const folder = await invoke<string | null>('pick_codex_home_folder');
      if (!folder) return;
      await saveCodexSafe({ ...codexConfig, codexHomePath: folder, codexHomeMode: 'clawscribe-isolated', useExistingUserCodexSession: false, codexBinaryPath: null });
    } catch (err) {
      toast.error(typeof err === 'string' ? err : 'Could not use that folder');
    }
  };

  const saveCodexSafe = async (config: CodexProviderConfig) => {
    try { return await saveCodexConfig(config); } catch { throw new SaveError('Could not save the Codex settings'); }
  };

  const chooseCodexModel = (model: string) => {
    void feedback.run('model', async () => {
      const saved = await saveCodexSafe({ ...codexConfig, model });
      const next: ModelConfig = { ...modelConfig, model: saved.model, apiKey: null };
      const held = await saveConfig(next, 'Could not save the model');
      setModelConfig(next);
      return held;
    });
  };

  const commitApiKey = async (value: string): Promise<CommitResult> => {
    const next: ModelConfig = { ...modelConfig, apiKey: newKeyPayload(value) };
    const held = await saveConfig(next, 'Could not save the API key');
    // Kept with the preview. It is stored when the user switches to this provider.
    if (held) { setApiKey(value); setModelConfig(next); return held; }
    // Stored now: forget what was typed and show what the core reports.
    await afterKeySaved(modelConfig.provider);
    if (modelConfig.provider === 'openai') {
      try {
        const status = (await invoke('api_save_openai_auth_config', { config: { mode: 'api_key' } })) as OpenAIAuthStatus;
        setOpenAIAuthStatus(status);
      } catch {
        throw new SaveError('The key is saved, but the OpenAI sign-in mode could not be updated');
      }
    }
  };

  const customValues = () => ({
    endpoint: customOpenAIEndpoint, model: customOpenAIModel, apiKey: customOpenAIApiKey, contextWindow: customContextWindow,
    maxTokens: customMaxTokens, temperature: customTemperature, topP: customTopP, timeoutSeconds: customTimeoutSeconds,
    organization: customOrganization, project: customProject, allowUnencrypted: customAllowUnencrypted,
  });

  const commitCustom = async (over: Partial<ReturnType<typeof customValues>>): Promise<CommitResult> => {
    const v = { ...customValues(), ...over };
    if (keyRequiredForAddress(savedCustomEndpoint, v.endpoint.trim() || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT, customKeyStatus, v.apiKey)) {
      // Keep the new address; it is saved together with the key.
      setCustomOpenAIEndpoint(v.endpoint);
      return 'held';
    }
    const payload = {
      endpoint: v.endpoint.trim() || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT,
      apiKey: newKeyPayload(v.apiKey),
      model: v.model.trim() || DEFAULT_OPENAI_COMPATIBLE_MODEL,
      maxTokens: v.maxTokens ? parseInt(v.maxTokens, 10) : null,
      contextWindow: Number(v.contextWindow) || 8192,
      temperature: v.temperature ? parseFloat(v.temperature) : null,
      topP: v.topP ? parseFloat(v.topP) : null,
      timeoutSeconds: v.timeoutSeconds ? parseInt(v.timeoutSeconds, 10) : null,
      organization: v.organization.trim() || null,
      project: v.project.trim() || null,
    };
    try {
      await invoke('api_save_custom_openai_config', { allowUnencrypted: v.allowUnencrypted, ...payload });
    } catch (err) {
      throw new SaveError(safeMessage(err, 'Could not save the connection settings', [v.apiKey, v.organization, v.project]));
    }
    const next: ModelConfig = {
      ...modelConfig, provider: 'custom-openai', apiKey: null, model: payload.model,
      customOpenAIEndpoint: payload.endpoint, customOpenAIModel: payload.model, customOpenAIApiKey: null,
      contextWindow: payload.contextWindow, maxTokens: payload.maxTokens, temperature: payload.temperature, topP: payload.topP,
      timeoutSeconds: payload.timeoutSeconds, organization: payload.organization, project: payload.project,
    };
    const held = await saveConfig(next, 'Could not save the model settings');
    rememberModel('custom-openai', payload.model);
    setModelConfig(next);
    setCustomAllowUnencrypted(v.allowUnencrypted);
    setCustomDestinationProblem(null);
    setCustomCheck({ state: 'idle', message: '' });
    setSavedCustomEndpoint(payload.endpoint);
    if (payload.apiKey) { setCustomOpenAIApiKey(''); await refreshCustomKeyStatus(); await refreshProviderKeyStatus?.('custom-openai'); }
    return held;
  };

  const commitOllamaEndpoint = async (value: string): Promise<CommitResult> => {
    const next: ModelConfig = { ...modelConfig, apiKey: null, ollamaEndpoint: value || null };
    const held = await saveConfig(next, 'Could not save the endpoint');
    setModelConfig(next);
    setOllamaEndpoint(value);
    void fetchOllamaModels(true, value);
    return held;
  };

  const modelListLoading =
    (modelConfig.provider === 'openrouter' && isLoadingOpenRouter) || (catalog.provider === modelConfig.provider && catalog.status === 'loading') ||
    (modelConfig.provider === 'ollama' && isLoadingOllama);

  const runCustomCheck = async () => {
    if (!customOpenAIEndpoint.trim() || !customOpenAIModel.trim()) {
      setCustomCheck({ state: 'error', message: 'Enter an API base URL and a model name first' });
      return;
    }
    setCustomCheck({ state: 'checking', message: '' });
    try {
      const result = await invoke<{ status: string; message: string }>('api_test_custom_openai_connection', {
        allowUnencrypted: customAllowUnencrypted,
        ...customOpenAIInvokeConfig(),
      });
      setCustomCheck({ state: 'ok', message: result.message || 'Connected' });
    } catch (err) {
      setCustomCheck({ state: 'error', message: safeMessage(err, 'Could not connect', [customOpenAIApiKey, customOrganization, customProject]) });
    }
  };

  const claudeStatus = claudeCode.status;
  const claudeBusy = claudeCode.busy;
  let isWindows = false;
  try { isWindows = osPlatform() === 'windows'; } catch { /* the guide also works when the platform is unknown */ }

  type ProviderStatus = { tone: StatusTone; text: string; meta?: string };
  const providerStatus = (): ProviderStatus => {
    const provider = modelConfig.provider;
    if (provider === 'builtin-ai') return modelConfig.model
      ? { tone: 'ok', text: 'Runs on this PC', meta: 'Offline' }
      : { tone: 'neutral', text: 'No summary model selected' };
    if (provider === 'ollama') {
      if (isLoadingOllama) return { tone: 'neutral', text: 'Checking Ollama…' };
      if (ollamaNotInstalled) return { tone: 'error', text: 'Ollama is not installed or not running' };
      if (error) return { tone: 'error', text: 'Could not reach Ollama' };
      if (models.length > 0) return { tone: 'ok', text: 'Connected', meta: `${models.length} ${models.length === 1 ? 'model' : 'models'}` };
      return { tone: 'warning', text: 'No models found' };
    }
    if (needsKey(provider)) {
      if (!hasUsableKey(apiKey, keyStatus)) return { tone: 'warning', text: 'Needs an API key' };
      if (modelListLoading) return { tone: 'neutral', text: 'Loading models…' };
      if (provider === 'openrouter') return openRouterError
        ? { tone: 'warning', text: 'Public model list unavailable' }
        : { tone: 'neutral', text: 'API key not verified by the public model list' };
      if (catalog.provider !== provider) return { tone: 'neutral', text: 'Checking provider…' };
      switch (catalog.status) {
        case 'invalid': return { tone: 'error', text: 'The provider rejected this API key. Replace it and try again.' };
        case 'forbidden': return { tone: 'warning', text: 'This key cannot list models. Check its permissions.' };
        case 'unavailable': return { tone: 'warning', text: 'Could not verify this key. Showing default models.' };
        case 'empty': return { tone: 'warning', text: 'The provider returned no supported chat models.' };
        case 'ready': return { tone: 'ok', text: 'Model list verified with this key' };
        default: return { tone: 'neutral', text: 'Key not checked yet' };
      }
    }
    if (provider === 'custom-openai') {
      if (customCheck.state === 'checking') return { tone: 'neutral', text: 'Checking connection…' };
      if (customCheck.state === 'ok') return { tone: 'ok', text: 'Connected' };
      if (customCheck.state === 'error') return { tone: 'error', text: 'Could not connect' };
      return { tone: 'neutral', text: 'Not checked yet' };
    }
    if (provider === 'codex') {
      if (isCodexBusy) return { tone: 'neutral', text: 'Checking…' };
      if (!codexStatus) return { tone: 'neutral', text: 'Not checked yet' };
      return codexStatus.found
        ? { tone: 'ok', text: 'Runtime found', meta: [codexStatus.version, codexStatus.planType].filter(Boolean).join(' · ') || undefined }
        : { tone: 'warning', text: 'Runtime missing or damaged' };
    }
    // claude-code
    if (claudeBusy) return { tone: 'neutral', text: 'Checking Claude Code…' };
    if (!claudeStatus) return { tone: 'neutral', text: 'Not checked yet' };
    if (claudeStatus.state === 'ready') {
      const version = claudeStatus.version.includes('Claude Code') ? claudeStatus.version : `Claude Code ${claudeStatus.version}`;
      return { tone: 'ok', text: 'Ready', meta: [version, claudeStatus.plan ? `${claudeStatus.plan} plan` : null].filter(Boolean).join(' · ') };
    }
    if (claudeStatus.state === 'not_installed') return { tone: 'warning', text: 'Claude Code is not installed' };
    if (claudeStatus.state === 'not_signed_in') return { tone: 'warning', text: 'Not signed in to Claude Code' };
    return { tone: 'error', text: 'Could not check Claude Code' };
  };

  const checkingNow =
    (modelConfig.provider === 'claude-code' && claudeBusy) || (modelConfig.provider === 'codex' && isCodexBusy) ||
    (modelConfig.provider === 'ollama' && isLoadingOllama) ||
    (modelConfig.provider === 'custom-openai' && customCheck.state === 'checking') || (needsKey(modelConfig.provider) && modelListLoading);

  const checkAgain = async () => {
    const provider = modelConfig.provider;
    if (provider === 'claude-code') return claudeCode.check();
    if (provider === 'codex') return checkCodexInstallation(codexConfig);
    if (provider === 'ollama') return fetchOllamaModels(true, ollamaEndpoint);
    if (provider === 'custom-openai') return runCustomCheck();
    if (provider === 'openai') await loadOpenAIAuthStatus();
    if (provider === 'openai' || provider === 'claude' || provider === 'groq') return refreshHostedCatalog();
    if (provider === 'openrouter') { setOpenRouterModels([]); setOpenRouterError(''); return loadOpenRouterModels(); }
  };

  const modelError = feedback.states.model?.status === 'error' ? feedback.states.model.message : null;

  const renderModelPicker = (key: string) => (
    <SettingsRow key={key} title="Model" htmlFor="summary-model" description={withError(null, modelError)}>
      <SaveMark status={feedback.states.model?.status} />
      <Popover open={modelComboboxOpen} onOpenChange={setModelComboboxOpen} modal={true}>
        <PopoverTrigger asChild>
          <Button id="summary-model" type="button" variant="outline" role="combobox" aria-expanded={modelComboboxOpen} disabled={feedback.busy}
            className={cn(ROW_SELECT, 'justify-between font-normal')}>
            <span className="truncate">{modelConfig.model || (modelListLoading ? 'Loading models…' : 'Select a model')}</span>
            <ChevronsUpDown aria-hidden="true" className="h-4 w-4 shrink-0 opacity-50" />
          </Button>
        </PopoverTrigger>
        <PopoverContent className="w-[250px] p-0" align="end">
          <Command>
            <CommandInput placeholder="Search models" />
            <CommandList className="max-h-[300px]">
              {modelListLoading ? (
                <div role="status" className="py-6 text-center text-sm text-muted-foreground">
                  <RefreshCw aria-hidden="true" className="mx-auto mb-2 h-4 w-4 animate-spin" />
                  Loading models
                </div>
              ) : (
                <>
                  <CommandEmpty>No models found</CommandEmpty>
                  <CommandGroup>
                    {modelOptions[modelConfig.provider]?.map((model) => (
                      <CommandItem key={model} value={model} onSelect={(value) => { setModelComboboxOpen(false); chooseModelAutosave(value); }}>
                        <Check aria-hidden="true" className={cn('mr-2 h-4 w-4', modelConfig.model === model ? 'opacity-100' : 'opacity-0')} />
                        <span className="truncate">{model}</span>
                      </CommandItem>
                    ))}
                  </CommandGroup>
                </>
              )}
            </CommandList>
          </Command>
        </PopoverContent>
      </Popover>
    </SettingsRow>
  );

  const renderSimpleModelSelect = (key: string, options: { value: string; label: string }[], onChange: (value: string) => void, loading?: boolean) => (
    <SettingsRow key={key} title="Model" htmlFor="summary-model" description={withError(null, modelError)}>
      <SaveMark status={feedback.states.model?.status} />
      <Select value={modelConfig.model} onValueChange={onChange} disabled={feedback.busy || loading}>
        <SelectTrigger id="summary-model" aria-label="Model" className={ROW_SELECT}>
          <SelectValue placeholder={loading ? 'Loading models…' : 'Select a model'} />
        </SelectTrigger>
        <SelectContent>
          {options.map((option) => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}
        </SelectContent>
      </Select>
    </SettingsRow>
  );

  const renderProviderRows = () => {
    const provider = modelConfig.provider;
    const rows: ReactNode[] = [];
    const advanced: ReactNode[] = [];

    if (provider === 'builtin-ai') {
      rows.push(
        <div key="builtin" className="px-5 py-3">
          <BuiltInModelManager
            selectedModel={modelConfig.model}
            layout={layout}
            onModelSelect={(model) => chooseModelAutosave(model)}
          />
        </div>,
      );
    }

    if (needsKey(provider)) {
      rows.push(
        // While another provider is only being looked at, a typed key stays in the
        // preview; otherwise it is saved the moment it is entered.
        previewConfig ? (
          <SecretKeyRow
            key="api-key" id="summary-api-key" title="API key" status={keyStatus}
            draft={modelConfig.apiKey || ''} onDraftChange={(value) => setModelConfig((prev: ModelConfig) => ({ ...prev, apiKey: value || null }))}
            description="Protected by your operating system. It is saved when you use this provider"
          />
        ) : (
          <SecretKeyRow
            key="api-key" id="summary-api-key" title="API key" status={keyStatus}
            onCommit={commitApiKey} onRemove={removeStoredKey} removeWarning={removeWarning(provider)}
            description={provider === 'openai' ? 'For Codex sign-in, choose Codex app-server instead' : 'Protected by your operating system'}
          />
        ),
        renderModelPicker('model'),
      );
      if (provider === 'openai') {
        advanced.push(
          <SettingsRow key="openai-keys" title="Get an API key" description="Opens the OpenAI platform in your browser">
            <Button type="button" variant="outline" onClick={() => openExternal('https://platform.openai.com/api-keys')}>
              <ExternalLink aria-hidden="true" />Open API keys
            </Button>
          </SettingsRow>,
        );
      }
    }

    if (provider === 'ollama') {
      rows.push(renderModelPicker('model'));
      if (ollamaNotInstalled) {
        rows.push(
          <SettingsRow key="ollama-install" title="Ollama" description="Install Ollama, restart ClawScribe, then choose Check again">
            <Button type="button" variant="outline" onClick={() => openExternal('https://ollama.com/download')}>
              <ExternalLink aria-hidden="true" />Download Ollama
            </Button>
          </SettingsRow>,
        );
      } else if (!isLoadingOllama && models.length === 0) {
        const downloading = isDownloading('gemma3:1b');
        const progress = getProgress('gemma3:1b');
        rows.push(
          <SettingsRow key="ollama-recommended" title="Recommended model" description={`gemma3:1b, about 800 MB. ${ollamaEndpointChanged ? 'Check again to load models from the new endpoint' : 'No models are installed yet'}`}>
            {downloading && progress !== undefined && <Meta>{Math.round(progress)}%</Meta>}
            <Button type="button" variant="outline" onClick={downloadRecommendedModel} disabled={downloading || ollamaEndpointChanged}>
              {downloading ? <RefreshCw aria-hidden="true" className="animate-spin" /> : <Download aria-hidden="true" />}
              {downloading ? 'Downloading' : 'Download'}
            </Button>
          </SettingsRow>,
        );
        if (downloading && progress !== undefined) {
          rows.push(<div key="ollama-progress" className="px-5 py-3"><Progress aria-label="Model download progress" value={progress} /></div>);
        }
      }
      advanced.push(
        <CommitInputRow
          key="ollama-endpoint" id="summary-ollama-endpoint" title="Endpoint" type="url" placeholder="http://localhost:11434"
          description="Leave empty to use the local Ollama server" value={ollamaEndpoint} validate={(value) => (value ? urlProblem(value) : null)}
          onCommit={commitOllamaEndpoint}
        />,
      );
    }

    if (provider === 'custom-openai') {
      rows.push(
        <CommitInputRow
          key="custom-endpoint" id="custom-endpoint" title="API base URL" type="url" placeholder={DEFAULT_OPENAI_COMPATIBLE_ENDPOINT}
          description="Use another /v1 endpoint for OpenAI-compatible gateways" value={customOpenAIEndpoint}
          validate={(value) => (value ? urlProblem(value) : 'The API base URL is required')} onCommit={(value) => commitCustom({ endpoint: value })}
        />,
        <div key="custom-http" className="px-5 py-3 empty:hidden">
          <UnencryptedHttpOptIn destinationProblem={customDestinationProblem} urls={[customOpenAIEndpoint]} checked={customAllowUnencrypted}
            onChange={(value) => { void feedback.run('custom-http', async () => { await commitCustom({ allowUnencrypted: value }); }); }} />
        </div>,
        <CommitInputRow
          key="custom-model" id="custom-model" title="Model name" placeholder={DEFAULT_OPENAI_COMPATIBLE_MODEL}
          description="The model identifier sent with each request" value={customOpenAIModel}
          validate={requiredProblem('The model name')} onCommit={(value) => commitCustom({ model: value })}
        />,
        <SecretKeyRow
          key="custom-key" id="custom-api-key" title="API key" status={customKeyStatus} placeholder="Leave empty if not required"
          description="Protected by your operating system" onCommit={(value) => commitCustom({ apiKey: value })}
          required={customKeyRequired ? KEY_REQUIRED_FOR_NEW_ADDRESS : null}
          onRemove={removeCustomKey} removeWarning={removeWarning('custom-openai')}
        />,
      );
      if (customCheck.state === 'error') {
        rows.push(<NoteRow key="custom-error" tone="error">{customCheck.message}</NoteRow>);
      }
      advanced.push(
        <CommitInputRow key="custom-timeout" id="custom-timeout" title="Timeout" description="Seconds, 5 or more" value={customTimeoutSeconds}
          inputMode="numeric" validate={numberProblem(5, 86400, true, true)} onCommit={(value) => commitCustom({ timeoutSeconds: value })} />,
        <CommitInputRow key="custom-org" id="custom-organization" title="OpenAI organization" placeholder="org_..." description="Optional" value={customOrganization}
          onCommit={(value) => commitCustom({ organization: value })} />,
        <CommitInputRow key="custom-project" id="custom-project" title="OpenAI project" placeholder="proj_..." description="Optional" value={customProject}
          onCommit={(value) => commitCustom({ project: value })} />,
        <CommitInputRow key="custom-context" id="custom-context-window" title="Context window" description="Tokens. Longer meetings are summarized in sections to fit" value={customContextWindow}
          inputMode="numeric" validate={numberProblem(4096, 2000000, true, true)} onCommit={(value) => commitCustom({ contextWindow: value })} />,
        <CommitInputRow key="custom-max" id="custom-max-tokens" title="Maximum output tokens" placeholder="For example 4096" description="Optional" value={customMaxTokens}
          inputMode="numeric" validate={numberProblem(1, 2000000, true)} onCommit={(value) => commitCustom({ maxTokens: value })} />,
        <CommitInputRow key="custom-temp" id="custom-temperature" title="Temperature" placeholder="For example 0.7" description="0 to 2, optional" value={customTemperature}
          validate={numberProblem(0, 2, false)} onCommit={(value) => commitCustom({ temperature: value })} />,
        <CommitInputRow key="custom-top-p" id="custom-top-p" title="Top P" placeholder="For example 0.9" description="0 to 1, optional" value={customTopP}
          validate={numberProblem(0, 1, false)} onCommit={(value) => commitCustom({ topP: value })} />,
        <SettingsRow key="custom-actions" title="Tests and credentials"
          description={withError('Test a full meeting summary, or remove the saved key, organization and project', feedback.states['custom-clear']?.status === 'error' ? feedback.states['custom-clear'].message : null)}>
          <Button type="button" variant="outline" onClick={() => void testCustomOpenAIProcessing()} disabled={isTestingConnection || !customOpenAIEndpoint.trim() || !customOpenAIModel.trim()}>
            {isTestingConnection && <RefreshCw aria-hidden="true" className="animate-spin" />}Test meeting processing
          </Button>
          <Button type="button" variant="outline" disabled={feedback.busy} onClick={() => {
            void feedback.run('custom-clear', async () => {
              try { await removeCustomKey(); } catch { throw new SaveError('Could not remove the saved key'); }
              await commitCustom({ organization: '', project: '' });
            });
          }}>Clear credentials</Button>
        </SettingsRow>,
      );
    }

    if (provider === 'claude-code') {
      if (claudeBusy) rows.push(<NoteRow key="cc-busy" role="status">Checking Claude Code…</NoteRow>);
      if (!claudeBusy && claudeStatus?.state === 'not_installed') {
        rows.push(
          <SettingsRow key="cc-install" title="Install Claude Code" description={withError('Install Claude Code yourself, then choose Check again', claudeGuideError)}>
            <Button type="button" variant="outline" onClick={async () => {
              try { setClaudeGuideError(''); await openExternal('https://code.claude.com/docs/en/setup'); }
              catch { setClaudeGuideError('Could not open the install guide. Try again'); }
            }}>Open install guide</Button>
          </SettingsRow>,
        );
        if (isWindows) rows.push(<CopyCommandRow key="cc-winget" id="claude-code-command" title="Install command" command="winget install Anthropic.ClaudeCode" />);
      }
      if (!claudeBusy && claudeStatus?.state === 'not_signed_in') {
        rows.push(<CopyCommandRow key="cc-login" id="claude-code-command" title="Sign-in command" description="Run this in a terminal to sign in" command="claude auth login" />);
      }
      if (!claudeBusy && claudeStatus?.state === 'ready') {
        rows.push(renderSimpleModelSelect('model', CLAUDE_CODE_MODELS.map((alias) => ({ value: alias, label: alias.charAt(0).toUpperCase() + alias.slice(1) })), chooseModelAutosave));
      }
      if (!claudeBusy && claudeStatus?.state === 'error') rows.push(<NoteRow key="cc-error" tone="error">{claudeStatus.message}</NoteRow>);
      advanced.push(
        <CommitInputRow
          key="cc-path" id="claude-code-program" title="Program path" placeholder="Full path (optional)"
          description="Leave empty to find Claude Code automatically, or use the full path to the program or Windows command shim"
          value={claudeCode.programPath} disabled={claudeBusy}
          onCommit={async (value) => {
            try { await invoke('claude_code_save_config', { config: { programPath: value.trim() || null } }); }
            catch { throw new SaveError('Could not save the program path'); }
            claudeCode.setProgramPath(value);
            setClaudeRecheck((count) => count + 1);
          }}
        />,
      );
    }

    if (provider === 'codex') {
      rows.push(
        renderSimpleModelSelect(
          'model',
          codexPickerModels.map((model) => ({ value: model.id, label: `${model.displayName}${model.isDefault ? ' · recommended' : ''}` })),
          chooseCodexModel, isLoadingCodexModels,
        ),
        <SettingsRow key="codex-account" title="Account"
          description={codexModelError ? 'The model list is unavailable, so only the configured model is shown' : (codexStatus?.accountEmail || codexStatus?.authStatus || 'Not signed in')}>
          <Button type="button" variant="outline" onClick={() => void runCodexAction('codex_login_browser')} disabled={isCodexBusy}>
            <ExternalLink aria-hidden="true" />Sign in with ChatGPT
          </Button>
          <Button type="button" variant="outline" onClick={() => void runCodexAction('codex_logout')} disabled={isCodexBusy}>Sign out</Button>
        </SettingsRow>,
      );
      advanced.push(
        <SettingsRow key="codex-home" title="Isolated CODEX_HOME"
          description={<>
            A new or empty folder on a local drive that ClawScribe keeps its own Codex profile in. It never uses your normal ~/.codex profile or the standalone Codex CLI sign-in
            <Meta className="mt-1 block break-all">{codexConfig.codexHomePath || '%APPDATA%\\ClawScribe\\codex'}</Meta>
          </>}>
          <Button type="button" variant="outline" onClick={() => void chooseCodexHome()} disabled={isCodexBusy}>Choose folder</Button>
        </SettingsRow>,
        <CommitInputRow key="codex-timeout" id="codex-timeout" title="Timeout" description="Seconds, 30 or more" inputMode="numeric" value={String(codexConfig.timeoutSeconds)}
          validate={numberProblem(30, 86400, true, true)} onCommit={async (value) => { await saveCodexSafe({ ...codexConfig, timeoutSeconds: parseInt(value, 10) }); }} />,
        <SettingsRow key="codex-runtime" title="Runtime"
          description={codexStatus?.found ? `${codexStatus.version || 'Codex app-server runtime'} at ${codexStatus.path || 'bundled resources'}` : (codexStatus?.message || 'Bundled Codex runtime is missing or damaged. Repair or reinstall ClawScribe.')}>
          <Button type="button" variant="outline" onClick={() => void findCodexAutomatically()} disabled={isCodexBusy}>Check bundled runtime</Button>
          <Button type="button" variant="outline" onClick={() => void prepareCodexInstallRepair()} disabled={isCodexBusy}><Wrench aria-hidden="true" />Install or repair</Button>
        </SettingsRow>,
        <SettingsRow key="codex-tests" title="Tests" description="Check the app-server, or run a test meeting through it">
          <Button type="button" variant="outline" onClick={() => void runCodexAction('codex_test_app_server')} disabled={isCodexBusy}>Test app-server</Button>
          <Button type="button" variant="outline" onClick={() => void runCodexAction('codex_test_processing')} disabled={isCodexBusy}>Test meeting processing</Button>
        </SettingsRow>,
        <SettingsRow key="codex-device" title="Other sign-in" description="Use a device code when the browser sign-in is not available">
          <Button type="button" variant="outline" onClick={() => void runCodexAction('codex_login_device')} disabled={isCodexBusy}>Sign in with device code</Button>
          <Button type="button" variant="ghost" onClick={() => openExternal('https://github.com/ch3573r/ClawScribe/blob/main/docs/auth/codex-auth.md')}>
            <ExternalLink aria-hidden="true" />Auth docs
          </Button>
        </SettingsRow>,
        <SettingsRow key="codex-details" title="Details"
          description={<>
            <span className="block break-all">CODEX_HOME: {codexStatus?.codexHome || codexConfig.codexHomePath || '%APPDATA%\\ClawScribe\\codex'}</span>
            <span className="block">Runtime: {codexStatus?.runtimeKind || 'codex-app-server'} · Account: {codexStatus?.accountEmail || codexStatus?.authStatus || 'not signed in'}{codexStatus?.planType ? ` · Plan: ${codexStatus.planType}` : ''}{codexStatus?.rateLimitState ? ` · Rate limit: ${codexStatus.rateLimitState}` : ''}</span>
            {codexStatus?.runtimeSha256 && <span className="block break-all">SHA256: {codexStatus.runtimeSha256}</span>}
          </>} />,
      );
      if (codexLastResult) {
        advanced.push(<div key="codex-result" className="px-5 py-3"><pre className="max-h-40 overflow-auto whitespace-pre-wrap text-xs text-muted-foreground">{codexLastResult}</pre></div>);
      }
    }

    return { rows, advanced };
  };

  if (autosave) {
    const { rows, advanced } = renderProviderRows();
    const status = providerStatus();
    const providerState = feedback.states.provider;
    const noSummaryModel = sharedModelConfig.provider === 'builtin-ai' && !sharedModelConfig.model;
    return (
      <>
        <SettingsRow
          title="Provider"
          htmlFor="summary-provider"
          description={withError(PROVIDER_SUMMARY[modelConfig.provider], providerState?.status === 'error' ? providerState.message : null)}
        >
          <SaveMark status={providerState?.status} />
          <Select value={modelConfig.provider} onValueChange={changeProviderAutosave} disabled={feedback.busy}>
            <SelectTrigger id="summary-provider" aria-label="Provider" className={ROW_SELECT}>
              <SelectValue placeholder="Select a provider" />
            </SelectTrigger>
            <SelectContent className="max-h-72 overflow-y-auto">
              <SelectGroup>
                <SelectLabel className="normal-case">On your device</SelectLabel>
                <SelectItem value="builtin-ai">Built-in</SelectItem>
                <SelectItem value="ollama">Ollama</SelectItem>
              </SelectGroup>
              <SelectGroup>
                <SelectLabel className="normal-case">Cloud APIs</SelectLabel>
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
        {previewConfig && (
          <SettingsRow
            title="Not in use yet"
            description={draftProviderMissing() ?? (noSummaryModel
              ? 'Choose Use to save your summary provider.'
              : `${summaryProviderLabel(sharedModelConfig.provider)} stays your summary provider until you switch`)}
          >
            <Button type="button" variant="ghost" size="sm" disabled={feedback.busy} onClick={() => changeProviderAutosave(sharedModelConfig.provider)}>
              {noSummaryModel ? 'Cancel' : `Keep ${summaryProviderLabel(sharedModelConfig.provider)}`}
            </Button>
            <Button type="button" variant="outline" size="sm" disabled={feedback.busy || draftProviderMissing() !== null} onClick={useDraftProvider}>
              Use {summaryProviderLabel(modelConfig.provider)}
            </Button>
          </SettingsRow>
        )}
        {rows}
        {advancedOpen && advanced.length > 0 && <div id="summary-provider-advanced" className="divide-y divide-border/60">{advanced}</div>}
        <SettingsFooter
          actions={
            <>
              {advanced.length > 0 && (
                <Button type="button" variant="ghost" size="sm" aria-expanded={advancedOpen} aria-controls="summary-provider-advanced" onClick={() => setAdvancedOpen(!advancedOpen)}>
                  Advanced
                </Button>
              )}
              {modelConfig.provider !== 'builtin-ai' && (
                <Button type="button" variant="ghost" size="sm" disabled={checkingNow} onClick={() => void checkAgain()}>
                  {checkingNow ? 'Checking…' : modelConfig.provider === 'custom-openai' && customCheck.state === 'idle' ? 'Check connection' : 'Check again'}
                </Button>
              )}
            </>
          }
        >
          <span role="status"><StatusText tone={status.tone}>{status.text}</StatusText></span>
          {status.meta && <Meta>{status.meta}</Meta>}
        </SettingsFooter>
      </>
    );
  }

  return (
    <div>
      <div className="flex justify-between items-center mb-4">
        <h3 className="text-lg font-semibold">Model settings</h3>
      </div>

      <div className="space-y-4">
        <div>
          <Label>Summarization model</Label>
          <div className="flex space-x-2 mt-1">
            <Select
              value={modelConfig.provider}
              onValueChange={(value) => {
                const provider = value as ModelConfig['provider'];

                // Clear error state when switching providers
                setError('');

                // Save current provider's model to localStorage before switching
                const map = JSON.parse(localStorage.getItem('providerModelMap') || '{}');
                if (modelConfig.model) {
                  map[modelConfig.provider] = modelConfig.model;
                  localStorage.setItem('providerModelMap', JSON.stringify(map));
                }

                // Try to restore cached model for the new provider
                const savedModel = map[provider];
                const providerModels = modelOptions[provider];
                const defaultModel = providerModels && providerModels.length > 0
                  ? providerModels[0]
                  : '';
                const model = (savedModel && providerModels?.includes(savedModel))
                  ? savedModel
                  : defaultModel;

                setModelConfig({
                  ...modelConfig,
                  provider,
                  model,
                  apiKey: null,
                });

                // Load OpenRouter models only when OpenRouter is selected
                if (provider === 'openrouter') {
                  loadOpenRouterModels();
                }

                // Load Built-in AI models when selected
                if (provider === 'builtin-ai') {
                  loadBuiltinAiModels();
                }

                // Load custom OpenAI config when selected
                if (provider === 'custom-openai') {
                  invoke<CustomOpenAIConfig | null>('api_get_custom_openai_config').then((config) => {
                    if (config) {
                      setCustomOpenAIEndpoint(config.endpoint || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT);
                      setCustomOpenAIModel(config.model || DEFAULT_OPENAI_COMPATIBLE_MODEL);
                      setCustomAllowUnencrypted(config.allow_unencrypted ?? false);
                      setCustomDestinationProblem(config.destination_problem ?? null);
                      setCustomKeyStatus(readSecretStatus(config.apiKeyStatus));
                      setSavedCustomEndpoint(config.endpoint || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT);
                      setCustomMaxTokens(config.maxTokens?.toString() || '');
      setCustomContextWindow(config.contextWindow?.toString() || '8192');
                      setCustomTemperature(config.temperature?.toString() || '');
                      setCustomTopP(config.topP?.toString() || '');
                      setCustomTimeoutSeconds(config.timeoutSeconds?.toString() || DEFAULT_OPENAI_COMPATIBLE_TIMEOUT_SECONDS);
                      setCustomOrganization(config.organization || '');
                      setCustomProject(config.project || '');
                    }
                  }).catch((err) => {
                    console.error('Failed to load custom OpenAI config:', err);
                  });
                }
              }}
            >
              <SelectTrigger>
                <SelectValue placeholder="Select provider" />
              </SelectTrigger>
              <SelectContent className="max-h-72 overflow-y-auto">
                <SelectGroup>
                  <SelectLabel className="normal-case">On your device</SelectLabel>
                  <SelectItem value="builtin-ai">Built-in &middot; offline, no key</SelectItem>
                  <SelectItem value="ollama">Ollama &middot; local server</SelectItem>
                </SelectGroup>
                <SelectGroup>
                  <SelectLabel className="normal-case">Cloud APIs</SelectLabel>
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

            {modelConfig.provider !== 'builtin-ai' && modelConfig.provider !== 'custom-openai' && modelConfig.provider !== 'codex' && modelConfig.provider !== 'claude-code' && (
              <Popover open={modelComboboxOpen} onOpenChange={setModelComboboxOpen} modal={true}>
                <PopoverTrigger asChild>
                  <Button
                    variant="outline"
                    role="combobox"
                    aria-expanded={modelComboboxOpen}
                    className="flex-1 max-w-[200px] justify-between font-normal"
                  >
                    <span className="truncate">
                      {modelConfig.model || "Select model..."}
                    </span>
                    <ChevronsUpDown className="ml-2 h-4 w-4 shrink-0 opacity-50" />
                  </Button>
                </PopoverTrigger>
                <PopoverContent className="w-[250px] p-0" align="start">
                  <Command>
                    <CommandInput placeholder="Search models..." />
                    <CommandList className="max-h-[300px]">
                      {(modelConfig.provider === 'openrouter' && isLoadingOpenRouter) ||
                       (catalog.provider === modelConfig.provider && catalog.status === 'loading') ? (
                        <div className="py-6 text-center text-sm text-muted-foreground">
                          <RefreshCw className="mx-auto h-4 w-4 animate-spin mb-2" />
                          Loading models...
                        </div>
                      ) : (
                        <>
                          <CommandEmpty>No models found.</CommandEmpty>
                          <CommandGroup>
                            {modelOptions[modelConfig.provider]?.map((model) => (
                              <CommandItem
                                key={model}
                                value={model}
                                onSelect={(currentValue) => {
                                  setModelConfig((prev: ModelConfig) => ({ ...prev, model: currentValue }));
                                  setModelComboboxOpen(false);
                                }}
                              >
                                <Check
                                  className={cn(
                                    "mr-2 h-4 w-4",
                                    modelConfig.model === model ? "opacity-100" : "opacity-0"
                                  )}
                                />
                                <span className="truncate">{model}</span>
                              </CommandItem>
                            ))}
                          </CommandGroup>
                        </>
                      )}
                    </CommandList>
                  </Command>
                </PopoverContent>
              </Popover>
            )}
          </div>
        </div>

        {/* Custom OpenAI Configuration Section */}
        {modelConfig.provider === 'custom-openai' && (
          <div className="space-y-4 border-t pt-4">
            <div>
              <Label htmlFor="custom-endpoint">API base URL *</Label>
              <Input
                id="custom-endpoint"
                value={customOpenAIEndpoint}
                onChange={(e) => { setCustomOpenAIEndpoint(e.target.value); setCustomDestinationProblem(null); }}
                placeholder={DEFAULT_OPENAI_COMPATIBLE_ENDPOINT}
                className="mt-1"
              />
              <p className="text-xs text-muted-foreground mt-1">
                Defaults to the official OpenAI API. Use another /v1 endpoint for OpenAI-compatible gateways.
              </p>
            </div>

            <div>
              <UnencryptedHttpOptIn destinationProblem={customDestinationProblem} urls={[customOpenAIEndpoint]} checked={customAllowUnencrypted} onChange={value => { setCustomAllowUnencrypted(value); setCustomDestinationProblem(null); }} />
              <Label htmlFor="custom-model">Model name *</Label>
              <Input
                id="custom-model"
                value={customOpenAIModel}
                onChange={(e) => setCustomOpenAIModel(e.target.value)}
                placeholder={DEFAULT_OPENAI_COMPATIBLE_MODEL}
                className="mt-1"
              />
              <p className="text-xs text-muted-foreground mt-1">
                Model identifier to use for requests
              </p>
            </div>

            <div>
              <Label htmlFor="custom-api-key">API key</Label>
              <div className="mt-1">
                <SecretKeyControl
                  id="custom-api-key" status={customKeyStatus}
                  draft={customOpenAIApiKey} onDraftChange={setCustomOpenAIApiKey}
                  required={customKeyRequired ? KEY_REQUIRED_FOR_NEW_ADDRESS : null}
                  onRemove={removeCustomKey} removeWarning={removeWarning('custom-openai')}
                  placeholder="Leave empty if the endpoint does not require one"
                />
              </div>
              <p className="text-xs text-muted-foreground mt-1">
                Protected by your operating system. Leave empty only for gateways that do not require a token.
              </p>
            </div>

            {/* Advanced Options (Collapsible) */}
            <div>
              <div
                className="flex items-center justify-between cursor-pointer py-2"
                onClick={() => setIsCustomOpenAIAdvancedOpen(!isCustomOpenAIAdvancedOpen)}
              >
                <Label className="cursor-pointer">Advanced options</Label>
                {isCustomOpenAIAdvancedOpen ? (
                  <ChevronUp className="h-4 w-4 text-muted-foreground" />
                ) : (
                  <ChevronDown className="h-4 w-4 text-muted-foreground" />
                )}
              </div>

              {isCustomOpenAIAdvancedOpen && (
                <div className="space-y-3 pl-2 border-l-2 border-muted mt-2">
                  <div>
                    <Label htmlFor="custom-timeout">Timeout (seconds)</Label>
                    <Input
                      id="custom-timeout"
                      type="number"
                      min="5"
                      value={customTimeoutSeconds}
                      onChange={(e) => setCustomTimeoutSeconds(e.target.value)}
                      placeholder={DEFAULT_OPENAI_COMPATIBLE_TIMEOUT_SECONDS}
                      className="mt-1"
                    />
                  </div>
                  <div>
                    <Label htmlFor="custom-organization">OpenAI organization (optional)</Label>
                    <Input
                      id="custom-organization"
                      value={customOrganization}
                      onChange={(e) => setCustomOrganization(e.target.value)}
                      placeholder="org_..."
                      className="mt-1"
                    />
                  </div>
                  <div>
                    <Label htmlFor="custom-project">OpenAI project (optional)</Label>
                    <Input
                      id="custom-project"
                      value={customProject}
                      onChange={(e) => setCustomProject(e.target.value)}
                      placeholder="proj_..."
                      className="mt-1"
                    />
                  </div>
                  <div>
                    <Label htmlFor="custom-context-window">Context window (tokens)</Label>
                    <Input id="custom-context-window" type="number" min="4096" max="2000000"
                      value={customContextWindow} onChange={(event) => setCustomContextWindow(event.target.value)} className="mt-1" />
                    <p className="mt-1 text-xs text-muted-foreground">Use the context limit supported by your model. Longer meetings are summarized in sections to fit.</p>
                  </div>
                  <div>
                    <Label htmlFor="custom-max-tokens">Maximum output tokens</Label>
                    <Input
                      id="custom-max-tokens"
                      type="number"
                      value={customMaxTokens}
                      onChange={(e) => setCustomMaxTokens(e.target.value)}
                      placeholder="e.g., 4096"
                      className="mt-1"
                    />
                  </div>
                  <div>
                    <Label htmlFor="custom-temperature">Temperature (0.0-2.0)</Label>
                    <Input
                      id="custom-temperature"
                      type="number"
                      step="0.1"
                      min="0"
                      max="2"
                      value={customTemperature}
                      onChange={(e) => setCustomTemperature(e.target.value)}
                      placeholder="e.g., 0.7"
                      className="mt-1"
                    />
                  </div>
                  <div>
                    <Label htmlFor="custom-top-p">Top P (0.0-1.0)</Label>
                    <Input
                      id="custom-top-p"
                      type="number"
                      step="0.1"
                      min="0"
                      max="1"
                      value={customTopP}
                      onChange={(e) => setCustomTopP(e.target.value)}
                      placeholder="e.g., 0.9"
                      className="mt-1"
                    />
                  </div>
                </div>
              )}
            </div>

            <div className="grid gap-2 sm:grid-cols-3">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={testCustomOpenAIConnection}
                disabled={isTestingConnection || !customOpenAIEndpoint.trim() || !customOpenAIModel.trim()}
              >
                {isTestingConnection ? (
                  <RefreshCw className="mr-2 h-4 w-4 animate-spin" />
                ) : (
                  <CheckCircle2 className="mr-2 h-4 w-4" />
                )}
                Test connection
              </Button>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={testCustomOpenAIProcessing}
                disabled={isTestingConnection || !customOpenAIEndpoint.trim() || !customOpenAIModel.trim()}
              >
                {isTestingConnection ? (
                  <RefreshCw className="mr-2 h-4 w-4 animate-spin" />
                ) : (
                  <CheckCircle2 className="mr-2 h-4 w-4" />
                )}
                Test meeting processing
              </Button>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={clearCustomOpenAICredentials}
              >
                <XCircle className="mr-2 h-4 w-4" />
                Clear credentials
              </Button>
            </div>
          </div>
        )}

        {modelConfig.provider === 'claude-code' && (
          <ClaudeCodeProviderCard
            status={claudeCode.status}
            busy={claudeCode.busy}
            programPath={claudeCode.programPath}
            model={modelConfig.model}
            onProgramPathChange={claudeCode.setProgramPath}
            onModelChange={model => setModelConfig(prev => ({ ...prev, model }))}
            onCheck={claudeCode.check}
          />
        )}

        {modelConfig.provider === 'codex' && (
          <div className="space-y-4 border-t pt-4">
            <div className="flex items-start justify-between gap-3">
              <div className="space-y-1">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-medium">Advanced: Codex app-server</span>
                  <span className={cn(
                    'rounded-full px-2 py-0.5 text-xs font-medium',
                    codexStatus?.found ? 'bg-success text-success-foreground' : 'bg-warning text-warning-foreground'
                  )}>
                    {isCodexBusy ? 'Checking' : codexStatus?.found ? 'Bundled runtime found' : 'Missing or damaged'}
                  </span>
                </div>
                <p className="text-sm text-muted-foreground">
                  Codex app-server mode uses a bundled/pinned Codex runtime and ChatGPT/Codex sign-in. It does not use the Microsoft Store app executable and does not require Codex to be installed globally. For normal use without Codex, choose OpenAI API key.
                </p>
                <p className="text-xs text-muted-foreground">
                  {codexStatus?.found
                    ? `${codexStatus.version || 'Codex app-server runtime'} at ${codexStatus.path || 'bundled resources'}`
                    : codexStatus?.message || 'Bundled Codex runtime is missing or damaged. Repair/reinstall ClawScribe.'}
                </p>
                {codexStatus?.runtimeSha256 && (
                  <p className="text-xs text-muted-foreground">
                    Runtime SHA256: {codexStatus.runtimeSha256}
                  </p>
                )}
                <p className="text-xs text-muted-foreground">
                  CODEX_HOME: {codexStatus?.codexHome || codexConfig.codexHomePath || '%APPDATA%\\ClawScribe\\codex'}
                </p>
                <p className="text-xs text-muted-foreground">
                  Runtime: {codexStatus?.runtimeKind || 'codex-app-server'} · Account: {codexStatus?.accountEmail || codexStatus?.authStatus || 'not signed in'}{codexStatus?.planType ? ` · Plan: ${codexStatus.planType}` : ''}{codexStatus?.rateLimitState ? ` · Rate limit: ${codexStatus.rateLimitState}` : ''}
                </p>
                {codexStatus?.authStatus && (
                  <p className="text-xs text-muted-foreground">Auth state: {codexStatus.authStatus}</p>
                )}
              </div>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                onClick={() => checkCodexInstallation(codexConfig)}
                disabled={isCodexBusy}
                title="Check Codex installation"
              >
                <RefreshCw className={cn('h-4 w-4', isCodexBusy && 'animate-spin')} />
              </Button>
            </div>

            <div>
              <Label htmlFor="codex-home-path">Isolated CODEX_HOME</Label>
              <div className="mt-1 flex items-center gap-2">
                <span id="codex-home-path" className="min-w-0 flex-1 break-all text-sm">{codexConfig.codexHomePath || '%APPDATA%\\ClawScribe\\codex'}</span>
                <Button type="button" variant="outline" onClick={() => void chooseCodexHome()} disabled={isCodexBusy}>Choose folder</Button>
              </div>
              <p className="text-xs text-muted-foreground mt-1">
                ClawScribe never uses the user&apos;s normal ~/.codex profile or the standalone Codex CLI auth state for this provider.
              </p>
            </div>

            <div className="grid grid-cols-1 gap-3 sm:grid-cols-[1fr_140px]">
              <div>
                <Label htmlFor="codex-model">Codex model</Label>
                <Select
                  value={codexConfig.model}
                  onValueChange={(model) => {
                    setCodexConfig((prev) => ({ ...prev, model }));
                    setModelConfig((prev: ModelConfig) => ({ ...prev, model }));
                  }}
                  disabled={isLoadingCodexModels}
                >
                  <SelectTrigger id="codex-model" className="mt-1">
                    <SelectValue placeholder={isLoadingCodexModels ? 'Loading models...' : 'Select Codex model'} />
                  </SelectTrigger>
                  <SelectContent>
                    {codexPickerModels.map((model) => (
                      <SelectItem key={model.id} value={model.id}>
                        {model.displayName} · {model.id}{model.isDefault ? ' · recommended' : ''}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <p className="mt-1 text-xs text-muted-foreground">
                  {isLoadingCodexModels
                    ? 'Loading the model catalog from Codex app-server...'
                    : codexModelError
                      ? `The Codex model catalog is unavailable (${codexModelError}); only the configured model is shown.`
                      : 'Models are loaded from the bundled Codex app-server catalog.'}
                </p>
              </div>
              <div>
                <Label htmlFor="codex-timeout">Timeout</Label>
                <Input
                  id="codex-timeout"
                  type="number"
                  min="30"
                  value={codexConfig.timeoutSeconds}
                  onChange={(e) => setCodexConfig((prev) => ({ ...prev, timeoutSeconds: parseInt(e.target.value, 10) || 600 }))}
                  className="mt-1"
                />
              </div>
            </div>

            <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
              <Button type="button" variant="outline" onClick={findCodexAutomatically} disabled={isCodexBusy}>
                <RefreshCw className={cn('mr-2 h-4 w-4', isCodexBusy && 'animate-spin')} />
                Check bundled runtime
              </Button>
              <Button type="button" variant="outline" onClick={prepareCodexInstallRepair} disabled={isCodexBusy}>
                <Wrench className="mr-2 h-4 w-4" />
                Install/repair app-server
              </Button>
              <Button type="button" variant="outline" onClick={() => runCodexAction('codex_test_app_server')} disabled={isCodexBusy}>
                <CheckCircle2 className="mr-2 h-4 w-4" />
                Test app-server
              </Button>
              <Button type="button" variant="outline" onClick={() => runCodexAction('codex_test_processing')} disabled={isCodexBusy}>
                <CheckCircle2 className="mr-2 h-4 w-4" />
                Test meeting processing
              </Button>
              <Button type="button" variant="outline" onClick={() => runCodexAction('codex_login_browser')} disabled={isCodexBusy}>
                <ExternalLink className="mr-2 h-4 w-4" />
                Sign in with ChatGPT
              </Button>
              <Button type="button" variant="outline" onClick={() => runCodexAction('codex_login_device')} disabled={isCodexBusy}>
                <CheckCircle2 className="mr-2 h-4 w-4" />
                Sign in with device code
              </Button>
              <Button type="button" variant="outline" onClick={() => runCodexAction('codex_logout')} disabled={isCodexBusy}>
                <Lock className="mr-2 h-4 w-4" />
                Logout
              </Button>
            </div>

            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => openExternal('https://github.com/ch3573r/ClawScribe/blob/main/docs/auth/codex-auth.md')}
            >
              <ExternalLink className="mr-2 h-4 w-4" />
              Open Codex auth docs
            </Button>

            {codexLastResult && (
              <Alert>
                <AlertDescription>
                  <pre className="max-h-40 overflow-auto whitespace-pre-wrap text-xs">{codexLastResult}</pre>
                </AlertDescription>
              </Alert>
            )}
          </div>
        )}

        {requiresApiKey && (
          <div>
            <Label htmlFor="legacy-api-key">API key</Label>
            <div className="mt-1">
              <SecretKeyControl
                id="legacy-api-key" status={keyStatus}
                draft={apiKey || ''} onDraftChange={(value) => setApiKey(value || null)}
                onRemove={removeStoredKey} removeWarning={removeWarning(modelConfig.provider)}
              />
              <p role="status" className="text-xs text-muted-foreground">{providerStatus().text}</p>
            </div>
          </div>
        )}

        {modelConfig.provider === 'openai' && (
          <Alert className="border-primary bg-primary/10">
            <AlertDescription className="text-primary">
              <div className="space-y-2">
                <div className="flex items-start justify-between gap-3">
                  <div>
                    <div className="font-medium">
                      OpenAI API key
                    </div>
                    <p className="text-sm">
                      Paste a key from the OpenAI platform. For standalone Codex app-server mode, choose Advanced: Codex app-server.
                    </p>
                  </div>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    onClick={loadOpenAIAuthStatus}
                    disabled={isLoadingOpenAIAuthStatus}
                    title="Refresh OpenAI auth status"
                  >
                    <RefreshCw className={cn('h-4 w-4', isLoadingOpenAIAuthStatus && 'animate-spin')} />
                  </Button>
                </div>
                <p className="text-xs">
                  {isLoadingOpenAIAuthStatus
                    ? 'Checking saved API-key status...'
                    : openAIAuthStatus?.apiKeyPresent
                      ? 'An OpenAI API key is saved.'
                      : 'OAuth metadata is informational only here; API requests still need an API key or an OpenAI-compatible gateway.'}
                </p>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => openExternal('https://platform.openai.com/api-keys')}
                >
                  <ExternalLink className="mr-2 h-4 w-4" />
                  Open API keys
                </Button>
              </div>
            </AlertDescription>
          </Alert>
        )}

        {modelConfig.provider === 'ollama' && (
          <div>
            <div
              className="flex items-center justify-between cursor-pointer py-2"
              onClick={() => setIsEndpointSectionCollapsed(!isEndpointSectionCollapsed)}
            >
              <Label className="cursor-pointer">Custom endpoint (optional)</Label>
              {isEndpointSectionCollapsed ? (
                <ChevronDown className="h-4 w-4 text-muted-foreground" />
              ) : (
                <ChevronUp className="h-4 w-4 text-muted-foreground" />
              )}
            </div>

            {!isEndpointSectionCollapsed && (
              <>
                <p className="text-sm text-muted-foreground mt-1 mb-2">
                  Leave empty or enter a custom endpoint (e.g., http://x.yy.zz:11434)
                </p>
                <div className="flex gap-2 mt-1">
                  <div className="relative flex-1">
                    <Input
                      type="url"
                      value={ollamaEndpoint}
                      onChange={(e) => {
                        setOllamaEndpoint(e.target.value);
                        // Clear models and errors when endpoint changes to avoid showing stale data
                        if (e.target.value.trim() !== lastFetchedEndpoint.trim()) {
                          setModels([]);
                          setError(''); // Clear error state
                        }
                      }}
                      placeholder="http://localhost:11434"
                      className={cn(
                        "pr-10",
                        endpointValidationState === 'invalid' && "border-destructive"
                      )}
                    />
                    {endpointValidationState === 'valid' && (
                      <CheckCircle2 className="absolute right-3 top-1/2 -translate-y-1/2 h-5 w-5 text-success-foreground" />
                    )}
                    {endpointValidationState === 'invalid' && (
                      <XCircle className="absolute right-3 top-1/2 -translate-y-1/2 h-5 w-5 text-destructive" />
                    )}
                  </div>
                  <Button
                    type="button"
                    size={'sm'}
                    onClick={() => fetchOllamaModels(false, ollamaEndpoint)}
                    disabled={isLoadingOllama}
                    variant="outline"
                    className="whitespace-nowrap"
                  >
                    {isLoadingOllama ? (
                      <>
                        <RefreshCw className="mr-2 h-4 w-4 animate-spin" />
                        Fetching...
                      </>
                    ) : (
                      <>
                        <RefreshCw className="mr-2 h-4 w-4" />
                        Fetch models
                      </>
                    )}
                  </Button>
                </div>
                {ollamaEndpointChanged && !error && (
                  <Alert className="mt-3 border-warning-border bg-warning">
                    <AlertDescription className="text-warning-foreground">
                      Endpoint changed. Please click &quot;Fetch models&quot; to load models from the new endpoint before saving.
                    </AlertDescription>
                  </Alert>
                )}
              </>
            )}
          </div>
        )}

        {modelConfig.provider === 'ollama' && (
          <div>
            <div className="flex items-center justify-between mb-4">
              <h4 className="text-sm font-bold">Available Ollama models</h4>
              {lastFetchedEndpoint && models.length > 0 && (
                <div className="flex items-center gap-2 text-sm">
                  <span className="text-muted-foreground">Using:</span>
                  <code className="px-2 py-1 bg-muted rounded text-xs">
                    {lastFetchedEndpoint || 'http://localhost:11434'}
                  </code>
                </div>
              )}
            </div>
            {models.length > 0 && (
              <div className="mb-4">
                <Input
                  placeholder="Search models..."
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  className="w-full"
                />
              </div>
            )}
            {isLoadingOllama ? (
              <div className="text-center py-8 text-muted-foreground">
                <RefreshCw className="mx-auto h-8 w-8 animate-spin mb-2" />
                Loading models...
              </div>
            ) : models.length === 0 ? (
              <div className="space-y-3">
                {ollamaNotInstalled ? (
                  /* Show Ollama download link when not installed */
                  <div className="space-y-4">
                    <Alert className="border-warning-border bg-warning">
                      <AlertDescription className="text-warning-foreground">
                        Ollama is not installed or not running. Please download and install Ollama to use local models.
                      </AlertDescription>
                    </Alert>
                    <Button
                      variant="default"
                      size="sm"
                      onClick={() => openExternal('https://ollama.com/download')}
                      className="w-full bg-primary hover:bg-primary"
                    >
                      <ExternalLink className="mr-2 h-4 w-4" />
                      Download Ollama
                    </Button>
                    <div className="text-sm text-muted-foreground text-center">
                      After installing Ollama, restart this application and click &quot;Fetch models&quot; to continue.
                    </div>
                  </div>
                ) : (
                  /* Show model download option when Ollama is installed but no models */
                  <>
                    <Alert className="mb-4">
                      <AlertDescription>
                        {ollamaEndpointChanged
                          ? 'Endpoint changed. Click "Fetch models" to load models from the new endpoint.'
                          : 'No models found. Download a recommended model or click "Fetch models" to load available Ollama models.'}
                      </AlertDescription>
                    </Alert>
                    {!ollamaEndpointChanged && (
                      <div className="space-y-3">
                        <Button
                          variant="outline"
                          size="sm"
                          onClick={downloadRecommendedModel}
                          disabled={isDownloading('gemma3:1b')}
                          className="w-full"
                        >
                          {isDownloading('gemma3:1b') ? (
                            <>
                              <RefreshCw className="mr-2 h-4 w-4 animate-spin" />
                              Downloading gemma3:1b...
                            </>
                          ) : (
                            <>
                              <Download className="mr-2 h-4 w-4" />
                              Download gemma3:1b (recommended, ~800MB)
                            </>
                          )}
                        </Button>

                        {/* Show progress for gemma3:1b download */}
                        {isDownloading('gemma3:1b') && getProgress('gemma3:1b') !== undefined && (
                          <div className="bg-card rounded-md border p-3">
                            <div className="flex items-center justify-between mb-2">
                              <span className="text-sm font-medium text-primary">Downloading gemma3:1b</span>
                              <span className="text-sm font-semibold text-primary">
                                {Math.round(getProgress('gemma3:1b')!)}%
                              </span>
                            </div>
                            <div className="w-full h-2 bg-secondary rounded-full overflow-hidden">
                              <div
                                className="h-full bg-primary rounded-full transition-all duration-300"
                                style={{ width: `${getProgress('gemma3:1b')}%` }}
                              />
                            </div>
                          </div>
                        )}
                      </div>
                    )}
                  </>
                )}
              </div>
            ) : !ollamaEndpointChanged && (
              <ScrollArea className="max-h-[calc(100vh-450px)] overflow-y-auto pr-4">
                {filteredModels.length === 0 ? (
                  <Alert>
                    <AlertDescription>
                      No models found matching &quot;{searchQuery}&quot;. Try a different search term.
                    </AlertDescription>
                  </Alert>
                ) : (
                  <div className="grid gap-4">
                    {filteredModels.map((model) => {
                      const progress = getProgress(model.name);
                      const modelIsDownloading = isDownloading(model.name);

                      return (
                        <div
                          key={model.id}
                          className={cn(
                            'bg-card p-2 m-0 rounded-md border transition-colors',
                            modelConfig.model === model.name
                              ? 'ring-1 ring-ring border-primary bg-primary/10'
                              : 'hover:bg-muted/50',
                            !modelIsDownloading && 'cursor-pointer'
                          )}
                          onClick={() => {
                            if (!modelIsDownloading) {
                              setModelConfig((prev: ModelConfig) => ({ ...prev, model: model.name }))
                            }
                          }}
                        >
                          <div>
                            <b className="font-bold">{model.name}&nbsp;</b>
                            <span className="text-muted-foreground">with a size of </span>
                            <span className="font-mono font-bold text-sm">{model.size}</span>
                          </div>

                          {/* Progress bar for downloading models */}
                          {modelIsDownloading && progress !== undefined && (
                            <div className="mt-3 pt-3 border-t border-border">
                              <div className="flex items-center justify-between mb-2">
                                <span className="text-sm font-medium text-primary">Downloading...</span>
                                <span className="text-sm font-semibold text-primary">{Math.round(progress)}%</span>
                              </div>
                              <div className="w-full h-2 bg-secondary rounded-full overflow-hidden">
                                <div
                                  className="h-full bg-primary rounded-full transition-all duration-300"
                                  style={{ width: `${progress}%` }}
                                />
                              </div>
                            </div>
                          )}
                        </div>
                      );
                    })}
                  </div>
                )}
              </ScrollArea>
            )}
          </div>
        )}

        {/* Built-in AI Models Section */}
        {modelConfig.provider === 'builtin-ai' && (
          <div className="mt-6">
            <BuiltInModelManager
              selectedModel={modelConfig.model}
              layout={layout}
              onModelSelect={(model) =>
                setModelConfig((prev: ModelConfig) => ({ ...prev, model }))
              }
            />
          </div>
        )}
      </div>

      <div className="mt-6 flex justify-end">
        <Button
          className={cn(
            'px-4 text-sm font-medium text-primary-foreground rounded-md focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-ring',
            isDoneDisabled ? 'bg-muted cursor-not-allowed' : 'bg-primary hover:bg-primary'
          )}
          onClick={handleSave}
          disabled={isDoneDisabled}
        >
          Save
        </Button>
      </div>
    </div>
  );
}
