import { useState, useEffect, useRef } from 'react';
import { useSidebar } from './Sidebar/SidebarProvider';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import { useOllamaDownload } from '@/contexts/OllamaDownloadContext';
import { BuiltInModelManager } from '@/components/BuiltInModelManager';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { useConfig } from '@/contexts/ConfigContext';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Switch } from '@/components/ui/switch';
import { Lock, Unlock, Eye, EyeOff, RefreshCw, CheckCircle2, XCircle, ChevronDown, ChevronUp, Download, ExternalLink, Check, ChevronsUpDown, ServerCog, Wrench } from 'lucide-react';
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

export interface ModelConfig {
  provider: 'ollama' | 'groq' | 'claude' | 'openai' | 'openrouter' | 'builtin-ai' | 'custom-openai' | 'openclaw' | 'codex';
  model: string;
  whisperModel: string;
  apiKey?: string | null;
  ollamaEndpoint?: string | null;
  // Custom OpenAI fields
  customOpenAIEndpoint?: string | null;
  customOpenAIModel?: string | null;
  customOpenAIApiKey?: string | null;
  maxTokens?: number | null;
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

interface OpenAIModel {
  id: string;
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

interface OpenClawConfigStatus {
  enabled: boolean;
  configured: boolean;
  ready: boolean;
  bearer_token_configured: boolean;
  endpoint: string;
  model_endpoint: string;
  source: string;
  status_message: string;
  include_audio_path: boolean;
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

interface AnthropicModel {
  id: string;
  display_name?: string;
}

interface GroqModel {
  id: string;
  owned_by?: string;
}

// Fallback models for when API fetch fails or no API key provided
const OPENAI_FALLBACK_MODELS = [
  'gpt-4o',
  'gpt-4o-mini',
  'gpt-4-turbo',
  'gpt-4',
  'gpt-3.5-turbo',
  'o1',
  'o1-mini',
  'o3',
  'o3-mini',
];

const DEFAULT_OPENAI_COMPATIBLE_ENDPOINT = 'https://api.openai.com/v1';
const DEFAULT_OPENAI_COMPATIBLE_MODEL = 'gpt-4o-mini';
const DEFAULT_OPENAI_COMPATIBLE_TIMEOUT_SECONDS = '300';

const CLAUDE_FALLBACK_MODELS = [
  'claude-sonnet-4-5-20250929',
  'claude-haiku-4-5-20251001',
  'claude-opus-4-5-20251101',
  'claude-3-5-sonnet-latest',
];

const GROQ_FALLBACK_MODELS = [
  'llama-3.3-70b-versatile',
  'llama-3.1-70b-versatile',
  'mixtral-8x7b-32768',
  'gemma2-9b-it',
];

interface ModelSettingsModalProps {
  modelConfig: ModelConfig;
  setModelConfig: (config: ModelConfig | ((prev: ModelConfig) => ModelConfig)) => void;
  onSave: (config: ModelConfig) => void;
  skipInitialFetch?: boolean; // Optional: skip fetching config from backend if parent manages it
  layout?: 'inline' | 'dialog';
}

export function ModelSettingsModal({
  modelConfig: propsModelConfig,
  setModelConfig: propsSetModelConfig,
  onSave,
  skipInitialFetch = false,
  layout = 'inline',
}: ModelSettingsModalProps) {
  // Use ConfigContext if available, fallback to props for backward compatibility
  const configContext = useConfig();
  const modelConfig = configContext?.modelConfig || propsModelConfig;
  const setModelConfig = configContext?.setModelConfig || propsSetModelConfig;
  const providerApiKeys = configContext?.providerApiKeys;
  const updateProviderApiKey = configContext?.updateProviderApiKey;

  const [models, setModels] = useState<OllamaModel[]>([]);
  const [error, setError] = useState<string>('');
  const [apiKey, setApiKey] = useState<string | null>(modelConfig.apiKey || null);
  const [showApiKey, setShowApiKey] = useState<boolean>(false);
  const [isApiKeyLocked, setIsApiKeyLocked] = useState<boolean>(!!modelConfig.apiKey?.trim());
  const [isLockButtonVibrating, setIsLockButtonVibrating] = useState<boolean>(false);
  const { serverAddress } = useSidebar();
  const [openRouterModels, setOpenRouterModels] = useState<OpenRouterModel[]>([]);
  const [openRouterError, setOpenRouterError] = useState<string>('');
  const [isLoadingOpenRouter, setIsLoadingOpenRouter] = useState<boolean>(false);
  const [ollamaEndpoint, setOllamaEndpoint] = useState<string>(modelConfig.ollamaEndpoint || '');
  const [isLoadingOllama, setIsLoadingOllama] = useState<boolean>(false);
  const [lastFetchedEndpoint, setLastFetchedEndpoint] = useState<string>(modelConfig.ollamaEndpoint || '');
  const [endpointValidationState, setEndpointValidationState] = useState<'valid' | 'invalid' | 'none'>('none');
  const [hasAutoFetched, setHasAutoFetched] = useState<boolean>(false);
  const hasSyncedFromParent = useRef<boolean>(false);
  const hasLoadedInitialConfig = useRef<boolean>(false);
  const [autoGenerateEnabled, setAutoGenerateEnabled] = useState<boolean>(true); // Default to true
  const [searchQuery, setSearchQuery] = useState<string>('');
  const [isEndpointSectionCollapsed, setIsEndpointSectionCollapsed] = useState<boolean>(true); // Collapsed by default
  const [ollamaNotInstalled, setOllamaNotInstalled] = useState<boolean>(false); // Track if Ollama is not installed

  // Custom OpenAI state
  const [customOpenAIEndpoint, setCustomOpenAIEndpoint] = useState<string>(modelConfig.customOpenAIEndpoint || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT);
  const [customOpenAIModel, setCustomOpenAIModel] = useState<string>(modelConfig.customOpenAIModel || DEFAULT_OPENAI_COMPATIBLE_MODEL);
  const [customOpenAIApiKey, setCustomOpenAIApiKey] = useState<string>(modelConfig.customOpenAIApiKey || '');
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

  // Dynamic model fetching state for OpenAI, Claude, and Groq
  const [openaiModels, setOpenaiModels] = useState<string[]>([]);
  const [claudeModels, setClaudeModels] = useState<string[]>([]);
  const [groqModels, setGroqModels] = useState<string[]>([]);
  const [isLoadingOpenAI, setIsLoadingOpenAI] = useState<boolean>(false);
  const [isLoadingClaude, setIsLoadingClaude] = useState<boolean>(false);
  const [isLoadingGroq, setIsLoadingGroq] = useState<boolean>(false);
  const [openAIAuthStatus, setOpenAIAuthStatus] = useState<OpenAIAuthStatus | null>(null);
  const [isLoadingOpenAIAuthStatus, setIsLoadingOpenAIAuthStatus] = useState<boolean>(false);
  const [openClawStatus, setOpenClawStatus] = useState<OpenClawConfigStatus | null>(null);
  const [openClawStatusError, setOpenClawStatusError] = useState<string>('');
  const [isLoadingOpenClawStatus, setIsLoadingOpenClawStatus] = useState<boolean>(false);
  const [openClawEnabled, setOpenClawEnabled] = useState<boolean>(false);
  const [openClawEndpoint, setOpenClawEndpoint] = useState<string>('');
  const [openClawModelEndpoint, setOpenClawModelEndpoint] = useState<string>('');
  const [openClawBearerToken, setOpenClawBearerToken] = useState<string>('');
  const [openClawSource, setOpenClawSource] = useState<string>('ClawScribe');
  const [openClawIncludeAudioPath, setOpenClawIncludeAudioPath] = useState<boolean>(false);
  const [codexConfig, setCodexConfig] = useState<CodexProviderConfig>({
    codexHomeMode: 'clawscribe-isolated',
    codexHomePath: null,
    useExistingUserCodexSession: false,
    codexBinaryPath: null,
    model: 'gpt-5.5',
    timeoutSeconds: 600,
  });
  const [codexStatus, setCodexStatus] = useState<CodexInstallationStatus | null>(null);
  const [codexLastResult, setCodexLastResult] = useState<string>('');
  const [isCodexBusy, setIsCodexBusy] = useState<boolean>(false);

  // Use global download context instead of local state
  const { isDownloading, getProgress, downloadingModels } = useOllamaDownload();

  // Built-in AI models state
  const [builtinAiModels, setBuiltinAiModels] = useState<any[]>([]);

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

  const fetchApiKey = async (provider: string) => {
    try {
      const data = (await invoke('api_get_api_key', {
        provider,
      })) as string;
      setApiKey(data || '');
    } catch (err) {
      console.error('Error fetching API key:', err);
      setApiKey(null);
    }
  };

  // Auto-unlock when API key becomes empty, 
  useEffect(() => {
    const hasContent = !!apiKey?.trim();
    if (!hasContent) {
      setIsApiKeyLocked(false);
    }
  }, [apiKey]);

  const modelOptions: Record<string, string[]> = {
    ollama: models.map((model) => model.name),
    claude: claudeModels.length > 0 ? claudeModels : CLAUDE_FALLBACK_MODELS,
    groq: groqModels.length > 0 ? groqModels : GROQ_FALLBACK_MODELS,
    openai: openaiModels.length > 0 ? openaiModels : OPENAI_FALLBACK_MODELS,
    openrouter: openRouterModels.map((m) => m.id),
    'builtin-ai': builtinAiModels.map((m) => m.name),
    'custom-openai': [customOpenAIModel || DEFAULT_OPENAI_COMPATIBLE_MODEL],
    openclaw: ['openclaw-managed'],
    codex: [codexConfig.model || modelConfig.model || 'gpt-5.5'],
  };

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

  const isOpenClawInvalid = modelConfig.provider === 'openclaw' && (
    !openClawEndpoint.trim() ||
    !openClawModelEndpoint.trim() ||
    !openClawSource.trim() ||
    (!openClawBearerToken.trim() && !openClawStatus?.bearer_token_configured)
  );

  const isDoneDisabled =
    (requiresApiKey && (!apiKey || (typeof apiKey === 'string' && !apiKey.trim()))) ||
    (modelConfig.provider === 'ollama' && ollamaEndpointChanged) ||
    isCustomOpenAIInvalid ||
    isOpenClawInvalid;

  useEffect(() => {
    const fetchModelConfig = async () => {
      // If parent component manages config, skip fetch and just mark as loaded
      if (skipInitialFetch) {
        hasLoadedInitialConfig.current = true;
        return;
      }

      try {
        const data = (await invoke('api_get_model_config')) as any;
        if (data && data.provider !== null) {
          setModelConfig(data);

          // Fetch API key if not included in response and provider requires it
          if (data.provider !== 'ollama' && data.provider !== 'openclaw' && !data.apiKey) {
            try {
              const apiKeyData = await invoke('api_get_api_key', {
                provider: data.provider
              }) as string;
              data.apiKey = apiKeyData;
              setApiKey(apiKeyData);
            } catch (err) {
              console.error('Failed to fetch API key:', err);
            }
          }

          // Sync ollamaEndpoint state with fetched config
          if (data.ollamaEndpoint) {
            setOllamaEndpoint(data.ollamaEndpoint);
            // Don't set lastFetchedEndpoint here - it will be set after successful model fetch
          }
          hasLoadedInitialConfig.current = true; // Mark that initial config is loaded

          // Fetch Custom OpenAI config if that's the active provider
          if (data.provider === 'custom-openai') {
            try {
              const customConfig = (await invoke('api_get_custom_openai_config')) as any;
              if (customConfig) {
                setCustomOpenAIEndpoint(customConfig.endpoint || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT);
                setCustomOpenAIModel(customConfig.model || DEFAULT_OPENAI_COMPATIBLE_MODEL);
                setCustomOpenAIApiKey(customConfig.apiKey || '');
                setCustomMaxTokens(customConfig.maxTokens?.toString() || '');
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
        hasLoadedInitialConfig.current = true; // Mark as loaded even on error
      }
    };

    fetchModelConfig();
  }, [skipInitialFetch]);

  // Fetch auto-generate setting on mount
  useEffect(() => {
    const fetchAutoGenerateSetting = async () => {
      try {
        const enabled = (await invoke('api_get_auto_generate_setting')) as boolean;
        setAutoGenerateEnabled(enabled);
        console.log('Auto-generate setting loaded:', enabled);
      } catch (err) {
        console.error('Failed to fetch auto-generate setting:', err);
        // Keep default value (true) on error
      }
    };

    fetchAutoGenerateSetting();
  }, []);

  // Sync ollamaEndpoint state when modelConfig.ollamaEndpoint changes from parent
  useEffect(() => {
    const endpoint = modelConfig.ollamaEndpoint || '';
    if (endpoint !== ollamaEndpoint) {
      setOllamaEndpoint(endpoint);
      // Don't set lastFetchedEndpoint here - only after successful model fetch
    }
    // Only mark as synced if we have a valid provider (prevents race conditions during init)
    if (modelConfig.provider) {
      hasSyncedFromParent.current = true; // Mark that we've received prop value
    }
  }, [modelConfig.ollamaEndpoint, modelConfig.provider]);

  // Sync custom OpenAI state from modelConfig (context or props)
  useEffect(() => {
    if (modelConfig.provider === 'custom-openai') {
      console.log('Syncing custom OpenAI fields from ConfigContext:', {
        endpoint: modelConfig.customOpenAIEndpoint,
        model: modelConfig.customOpenAIModel,
        hasApiKey: !!modelConfig.customOpenAIApiKey,
      });

      // Always sync from modelConfig (which comes from context if available)
      setCustomOpenAIEndpoint(modelConfig.customOpenAIEndpoint || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT);
      setCustomOpenAIModel(modelConfig.customOpenAIModel || DEFAULT_OPENAI_COMPATIBLE_MODEL);
      setCustomOpenAIApiKey(modelConfig.customOpenAIApiKey || '');
      setCustomMaxTokens(modelConfig.maxTokens?.toString() || '');
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
    modelConfig.customOpenAIApiKey,
    modelConfig.maxTokens,
    modelConfig.temperature,
    modelConfig.topP,
    modelConfig.timeoutSeconds,
    modelConfig.organization,
    modelConfig.project
  ]);

  // Reset hasAutoFetched flag and clear models when switching away from Ollama
  useEffect(() => {
    if (modelConfig.provider !== 'ollama') {
      setHasAutoFetched(false); // Reset flag so it can auto-fetch again if user switches back
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
        setHasAutoFetched(false);
        setModels([]);
        setError('');
      }
    }
  }, [ollamaEndpoint, lastFetchedEndpoint, modelConfig.provider]);

  // Sync local apiKey state when provider changes
  useEffect(() => {
    if (providerApiKeys && requiresApiKey && modelConfig.provider !== 'custom-openai') {
      const correctKey = providerApiKeys[modelConfig.provider as keyof typeof providerApiKeys];
      if (correctKey !== apiKey) {
        setApiKey(correctKey || '');
        setIsApiKeyLocked(!!correctKey?.trim());
      }
    }
  }, [modelConfig.provider, providerApiKeys, requiresApiKey]);

  // Manual fetch function for Ollama models
  const fetchOllamaModels = async (silent = false) => {
    const trimmedEndpoint = ollamaEndpoint.trim();

    // Validate URL if provided
    if (trimmedEndpoint && !validateOllamaEndpoint(trimmedEndpoint)) {
      const errorMsg = 'Invalid Ollama endpoint URL. Must start with http:// or https://';
      setError(errorMsg);
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
      setModels(modelList);
      setLastFetchedEndpoint(trimmedEndpoint); // Track successful fetch

      // Cache the fetched models for this endpoint
      modelsCache.current.set(trimmedEndpoint, modelList);

      // Successfully fetched models, Ollama is installed
      setOllamaNotInstalled(false);
    } catch (err) {
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
      setIsLoadingOllama(false);
    }
  };

  // Auto-fetch models on initial load only (not on endpoint changes)
  useEffect(() => {
    let mounted = true;

    const initialLoad = async () => {
      // Only auto-fetch on initial load if:
      // 1. Provider is ollama
      // 2. Haven't fetched yet
      // 3. Component is still mounted
      // If skipInitialFetch is true, fetch silently (no error toasts)
      if (modelConfig.provider === 'ollama' &&
        !hasAutoFetched &&
        mounted) {
        await fetchOllamaModels(skipInitialFetch); // Silent if skipInitialFetch=true
        setHasAutoFetched(true);
      }
    };

    initialLoad();

    return () => {
      mounted = false;
    };
  }, [modelConfig.provider]); // Only depend on provider, NOT endpoint

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
      const data = (await invoke('builtin_ai_list_models')) as any[];
      setBuiltinAiModels(data);

      // Auto-select first available model if none selected
      if (data.length > 0 && !modelConfig.model) {
        const firstAvailable = data.find((m: any) => m.status?.type === 'available');
        if (firstAvailable) {
          setModelConfig((prev: ModelConfig) => ({ ...prev, model: firstAvailable.name }));
        }
      }
    } catch (err) {
      console.error('Error loading Built-in AI models:', err);
      toast.error('Failed to load Built-in AI models');
    }
  };

  // Fetch OpenAI models from API
  const loadOpenAIModels = async (key: string | null) => {
    if (!key?.trim()) {
      setOpenaiModels([]); // Will use fallback via modelOptions
      return;
    }
    setIsLoadingOpenAI(true);
    try {
      const data = (await invoke('get_openai_models', { apiKey: key })) as OpenAIModel[];
      setOpenaiModels(data.map((m) => m.id));
    } catch (err) {
      console.error('Error loading OpenAI models:', err);
      setOpenaiModels([]); // Will use fallback via modelOptions
    } finally {
      setIsLoadingOpenAI(false);
    }
  };

  // Fetch Anthropic (Claude) models from API
  const loadClaudeModels = async (key: string | null) => {
    if (!key?.trim()) {
      setClaudeModels([]); // Will use fallback via modelOptions
      return;
    }
    setIsLoadingClaude(true);
    try {
      const data = (await invoke('get_anthropic_models', { apiKey: key })) as AnthropicModel[];
      setClaudeModels(data.map((m) => m.id));
    } catch (err) {
      console.error('Error loading Claude models:', err);
      setClaudeModels([]); // Will use fallback via modelOptions
    } finally {
      setIsLoadingClaude(false);
    }
  };

  // Fetch Groq models from API
  const loadGroqModels = async (key: string | null) => {
    if (!key?.trim()) {
      setGroqModels([]); // Will use fallback via modelOptions
      return;
    }
    setIsLoadingGroq(true);
    try {
      const data = (await invoke('get_groq_models', { apiKey: key })) as GroqModel[];
      setGroqModels(data.map((m) => m.id));
    } catch (err) {
      console.error('Error loading Groq models:', err);
      setGroqModels([]); // Will use fallback via modelOptions
    } finally {
      setIsLoadingGroq(false);
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

  const loadOpenClawStatus = async () => {
    setIsLoadingOpenClawStatus(true);
    setOpenClawStatusError('');
    try {
      const status = (await invoke('get_openclaw_config_status')) as OpenClawConfigStatus;
      setOpenClawStatus(status);
      setOpenClawEnabled(status.enabled);
      setOpenClawEndpoint(status.endpoint || '');
      setOpenClawModelEndpoint(status.model_endpoint || '');
      setOpenClawSource(status.source || 'ClawScribe');
      setOpenClawIncludeAudioPath(status.include_audio_path);
      setOpenClawBearerToken('');
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      console.error('Error loading OpenClaw handoff status:', err);
      setOpenClawStatus(null);
      setOpenClawStatusError(message);
    } finally {
      setIsLoadingOpenClawStatus(false);
    }
  };

  const loadCodexConfig = async (): Promise<CodexProviderConfig | null> => {
    try {
      const config = (await invoke('codex_get_config')) as { processing?: { codex?: CodexProviderConfig } };
      const codex = config.processing?.codex;
      if (codex) {
        setCodexConfig(codex);
        if (codex.model && modelConfig.provider === 'codex') {
          setModelConfig((prev: ModelConfig) => ({ ...prev, model: codex.model }));
        }
        return codex;
      }
      return null;
    } catch (err) {
      console.error('Failed to load Codex config:', err);
      setCodexLastResult(err instanceof Error ? err.message : String(err));
      return null;
    }
  };

  const checkCodexInstallation = async (nextConfig?: CodexProviderConfig) => {
    setIsCodexBusy(true);
    try {
      if (nextConfig) {
        await saveCodexConfig(nextConfig);
      } else {
        await saveCodexConfig();
      }
      const status = (await invoke('codex_check_installation')) as CodexInstallationStatus;
      setCodexStatus(status);
      setCodexLastResult(status.message);
      if (status.found) {
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
  };

  const findCodexAutomatically = async () => {
    setIsCodexBusy(true);
    try {
      await saveCodexConfig({ ...codexConfig, codexBinaryPath: null });
      const status = (await invoke('codex_find_automatically')) as CodexInstallationStatus;
      setCodexStatus(status);
      setCodexLastResult(status.message);
      if (status.found && status.path) {
        setCodexConfig((prev) => ({ ...prev, codexBinaryPath: null }));
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

  const saveCodexConfig = async (nextConfig = codexConfig) => {
    const normalized = {
      ...nextConfig,
      codexHomeMode: 'clawscribe-isolated' as const,
      useExistingUserCodexSession: false,
      codexBinaryPath: null,
      model: nextConfig.model.trim() || 'gpt-5.5',
      timeoutSeconds: nextConfig.timeoutSeconds || 600,
    };
    const saved = (await invoke('codex_save_config', { config: normalized })) as { processing?: { codex?: CodexProviderConfig } };
    if (saved.processing?.codex) {
      setCodexConfig(saved.processing.codex);
    }
    return normalized;
  };

  const runCodexAction = async (command: 'codex_login_browser' | 'codex_login_device' | 'codex_logout' | 'codex_test_app_server' | 'codex_test_processing') => {
    setIsCodexBusy(true);
    try {
      await saveCodexConfig();
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

  // Auto-fetch OpenAI models when provider is openai and we have an API key
  useEffect(() => {
    if (modelConfig.provider === 'openai' && apiKey?.trim()) {
      loadOpenAIModels(apiKey);
    }
  }, [modelConfig.provider, apiKey]);

  useEffect(() => {
    if (modelConfig.provider === 'openai') {
      loadOpenAIAuthStatus();
    }
    if (modelConfig.provider === 'openclaw') {
      loadOpenClawStatus();
    }
    if (modelConfig.provider === 'codex') {
      loadCodexConfig().then((loaded) => checkCodexInstallation(loaded || undefined));
    }
  }, [modelConfig.provider]);

  // Auto-fetch Claude models when provider is claude and we have an API key
  useEffect(() => {
    if (modelConfig.provider === 'claude' && apiKey?.trim()) {
      loadClaudeModels(apiKey);
    }
  }, [modelConfig.provider, apiKey]);

  // Auto-fetch Groq models when provider is groq and we have an API key
  useEffect(() => {
    if (modelConfig.provider === 'groq' && apiKey?.trim()) {
      loadGroqModels(apiKey);
    }
  }, [modelConfig.provider, apiKey]);

  // Restore cached model when async model lists become available
  useEffect(() => {
    const providerModels = modelOptions[modelConfig.provider];
    if (!providerModels || providerModels.length === 0) return;

    // If current model is already valid, nothing to do
    if (modelConfig.model && providerModels.includes(modelConfig.model)) return;

    // Try to restore from localStorage cache
    const map = JSON.parse(localStorage.getItem('providerModelMap') || '{}');
    const cachedModel = map[modelConfig.provider];
    if (cachedModel && providerModels.includes(cachedModel)) {
      setModelConfig((prev: ModelConfig) => ({ ...prev, model: cachedModel }));
    }
  }, [models, openRouterModels, builtinAiModels, openaiModels, claudeModels, groqModels, modelConfig.provider]);

  const handleSave = async () => {
    // For custom-openai provider, save the custom config first
    if (modelConfig.provider === 'custom-openai') {
      try {
        await invoke('api_save_custom_openai_config', {
          endpoint: customOpenAIEndpoint.trim() || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT,
          apiKey: customOpenAIApiKey.trim() || null,
          model: customOpenAIModel.trim() || DEFAULT_OPENAI_COMPATIBLE_MODEL,
          maxTokens: customMaxTokens ? parseInt(customMaxTokens, 10) : null,
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

    let updatedConfig = {
      ...modelConfig,
      apiKey: typeof apiKey === 'string' ? apiKey.trim() || null : null,
      ollamaEndpoint: modelConfig.provider === 'ollama'
        ? (ollamaEndpoint.trim() || null)
        : (modelConfig.ollamaEndpoint || null),
      // Include custom OpenAI fields
      customOpenAIEndpoint: modelConfig.provider === 'custom-openai' ? (customOpenAIEndpoint.trim() || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT) : null,
      customOpenAIModel: modelConfig.provider === 'custom-openai' ? (customOpenAIModel.trim() || DEFAULT_OPENAI_COMPATIBLE_MODEL) : null,
      customOpenAIApiKey: modelConfig.provider === 'custom-openai' && customOpenAIApiKey.trim() ? customOpenAIApiKey.trim() : null,
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
    console.log('ModelSettingsModal - handleSave - Updated ModelConfig:', updatedConfig);

    // Persist confirmed model choice to per-provider cache
    if (updatedConfig.model) {
      const map = JSON.parse(localStorage.getItem('providerModelMap') || '{}');
      map[updatedConfig.provider] = updatedConfig.model;
      localStorage.setItem('providerModelMap', JSON.stringify(map));
    }

    // Update provider-specific key in context
    if (updateProviderApiKey && updatedConfig.apiKey && updatedConfig.provider !== 'custom-openai') {
      updateProviderApiKey(updatedConfig.provider, updatedConfig.apiKey);
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

    if (updatedConfig.provider === 'openclaw') {
      try {
        await invoke('save_openclaw_config', {
          config: {
            enabled: openClawEnabled,
            endpoint: openClawEndpoint.trim(),
            model_endpoint: openClawModelEndpoint.trim(),
            bearer_token: openClawBearerToken.trim(),
            source: openClawSource.trim(),
            include_audio_path: openClawIncludeAudioPath,
          },
        });
        setOpenClawBearerToken('');
        await loadOpenClawStatus();
      } catch (err) {
        const errorMsg = err instanceof Error ? err.message : String(err);
        console.error('Failed to save OpenClaw configuration:', err);
        toast.error(errorMsg || 'Failed to save OpenClaw configuration');
        return;
      }
    }

    if (updatedConfig.provider === 'codex') {
      try {
        const savedCodexConfig = await saveCodexConfig({
          ...codexConfig,
          model: codexConfig.model.trim() || updatedConfig.model || 'gpt-5.5',
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

    if (updatedConfig.provider === 'openai') {
      await loadOpenAIAuthStatus();
    }
  };

  const customOpenAIInvokeConfig = () => ({
    endpoint: customOpenAIEndpoint.trim() || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT,
    apiKey: customOpenAIApiKey.trim() || null,
    model: customOpenAIModel.trim() || DEFAULT_OPENAI_COMPATIBLE_MODEL,
    maxTokens: customMaxTokens ? parseInt(customMaxTokens, 10) : null,
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
        ...customOpenAIInvokeConfig(),
      });
      toast.success(result.message || 'Connection successful!');
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

  const clearCustomOpenAICredentials = () => {
    setCustomOpenAIApiKey('');
    setCustomOrganization('');
    setCustomProject('');
    toast.success('Credentials cleared from this form. Click Save to persist.');
  };

  const handleInputClick = () => {
    if (isApiKeyLocked) {
      setIsLockButtonVibrating(true);
      setTimeout(() => setIsLockButtonVibrating(false), 500);
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
      await fetchOllamaModels(true);

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
            onClick: () => invoke('open_external_url', { url: 'https://ollama.com/download' })
          }
        });
        // Update the installation status flag
        setOllamaNotInstalled(true);
      }
      // Other errors are handled by the context
    }
  };

  // Function to delete Ollama model
  const deleteOllamaModel = async (modelName: string) => {
    try {
      const endpoint = ollamaEndpoint.trim() || null;
      await invoke('delete_ollama_model', {
        modelName,
        endpoint
      });

      toast.success(`Model ${modelName} deleted`);
      await fetchOllamaModels(true); // Refresh list
    } catch (err) {
      const errorMsg = err instanceof Error ? err.message : 'Failed to delete model';
      toast.error(errorMsg);
      console.error('Error deleting model:', err);
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
      if (!current.has(modelName)) {
        // Download completed, refresh models list
        console.log(`[ModelSettingsModal] Download completed for ${modelName}, refreshing list`);
        fetchOllamaModels(true);
        break; // Only refresh once even if multiple completed
      }
    }

    // Update ref for next comparison
    previousDownloadingRef.current = new Set(current);
  }, [downloadingModels]);

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

  return (
    <div>
      <div className="flex justify-between items-center mb-4">
        <h3 className="text-lg font-semibold">Model Settings</h3>
      </div>

      <div className="space-y-4">
        <div>
          <Label>Summarization Model</Label>
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
                });
                // API key is now synced automatically via useEffect watching providerApiKeys

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
                  invoke<any>('api_get_custom_openai_config').then((config) => {
                    if (config) {
                      setCustomOpenAIEndpoint(config.endpoint || DEFAULT_OPENAI_COMPATIBLE_ENDPOINT);
                      setCustomOpenAIModel(config.model || DEFAULT_OPENAI_COMPATIBLE_MODEL);
                      setCustomOpenAIApiKey(config.apiKey || '');
                      setCustomMaxTokens(config.maxTokens?.toString() || '');
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

                if (provider === 'openclaw') {
                  loadOpenClawStatus();
                }

                if (provider === 'codex') {
                  loadCodexConfig().then((loaded) => checkCodexInstallation(loaded || undefined));
                }
              }}
            >
              <SelectTrigger>
                <SelectValue placeholder="Select provider" />
              </SelectTrigger>
              <SelectContent className="max-h-64 overflow-y-auto">
                <SelectItem value="custom-openai">OpenAI / OpenAI-compatible API</SelectItem>
                <SelectItem value="openclaw">OpenClaw</SelectItem>
                <SelectItem value="codex">Advanced: Codex app-server</SelectItem>
                <SelectItem value="builtin-ai">Built-in AI (Offline, No API needed)</SelectItem>
                <SelectItem value="ollama">Ollama</SelectItem>
                <SelectItem value="openrouter">OpenRouter</SelectItem>
                <SelectItem value="claude">Claude</SelectItem>
                <SelectItem value="groq">Groq</SelectItem>
              </SelectContent>
            </Select>

            {modelConfig.provider !== 'builtin-ai' && modelConfig.provider !== 'custom-openai' && modelConfig.provider !== 'openclaw' && modelConfig.provider !== 'codex' && (
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
                       (modelConfig.provider === 'openai' && isLoadingOpenAI) ||
                       (modelConfig.provider === 'claude' && isLoadingClaude) ||
                       (modelConfig.provider === 'groq' && isLoadingGroq) ? (
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
                onChange={(e) => setCustomOpenAIEndpoint(e.target.value)}
                placeholder={DEFAULT_OPENAI_COMPATIBLE_ENDPOINT}
                className="mt-1"
              />
              <p className="text-xs text-muted-foreground mt-1">
                Defaults to the official OpenAI API. Use another /v1 endpoint for OpenAI-compatible gateways.
              </p>
            </div>

            <div>
              <Label htmlFor="custom-model">Model Name *</Label>
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
              <Input
                id="custom-api-key"
                type="password"
                value={customOpenAIApiKey}
                onChange={(e) => setCustomOpenAIApiKey(e.target.value)}
                placeholder="Leave empty if the endpoint does not require one"
                className="mt-1"
              />
              <p className="text-xs text-muted-foreground mt-1">
                Stored in app settings and redacted from logs. Leave empty only for gateways that do not require a token.
              </p>
            </div>

            {/* Advanced Options (Collapsible) */}
            <div>
              <div
                className="flex items-center justify-between cursor-pointer py-2"
                onClick={() => setIsCustomOpenAIAdvancedOpen(!isCustomOpenAIAdvancedOpen)}
              >
                <Label className="cursor-pointer">Advanced Options</Label>
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
                    <Label htmlFor="custom-max-tokens">Max Tokens</Label>
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

        {modelConfig.provider === 'codex' && (
          <div className="space-y-4 border-t pt-4">
            <div className="flex items-start justify-between gap-3">
              <div className="space-y-1">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-medium">Advanced: Codex app-server</span>
                  <span className={cn(
                    'rounded-full px-2 py-0.5 text-xs font-medium',
                    codexStatus?.found ? 'bg-green-100 text-green-800' : 'bg-amber-100 text-amber-800'
                  )}>
                    {isCodexBusy ? 'Checking' : codexStatus?.found ? 'Bundled runtime found' : 'Missing or damaged'}
                  </span>
                </div>
                <p className="text-sm text-muted-foreground">
                  Codex app-server mode uses a bundled/pinned Codex runtime and ChatGPT/Codex sign-in. It does not use the Microsoft Store app executable and does not require Codex to be installed globally. For normal use without Codex, choose OpenAI API key or OpenClaw.
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
                onClick={() => checkCodexInstallation()}
                disabled={isCodexBusy}
                title="Check Codex installation"
              >
                <RefreshCw className={cn('h-4 w-4', isCodexBusy && 'animate-spin')} />
              </Button>
            </div>

            <div>
              <Label htmlFor="codex-home-path">Isolated CODEX_HOME</Label>
              <Input
                id="codex-home-path"
                value={codexConfig.codexHomePath || ''}
                onChange={(e) => setCodexConfig((prev) => ({
                  ...prev,
                  codexHomeMode: 'clawscribe-isolated',
                  useExistingUserCodexSession: false,
                  codexHomePath: e.target.value,
                  codexBinaryPath: null,
                }))}
                placeholder="%APPDATA%\\ClawScribe\\codex"
                className="mt-1"
              />
              <p className="text-xs text-muted-foreground mt-1">
                ClawScribe never uses the user's normal ~/.codex profile or the standalone Codex CLI auth state for this provider.
              </p>
            </div>

            <div className="grid grid-cols-1 gap-3 sm:grid-cols-[1fr_140px]">
              <div>
                <Label htmlFor="codex-model">Codex model</Label>
                <Input
                  id="codex-model"
                  value={codexConfig.model}
                  onChange={(e) => {
                    const model = e.target.value;
                    setCodexConfig((prev) => ({ ...prev, model }));
                    setModelConfig((prev: ModelConfig) => ({ ...prev, model }));
                  }}
                  placeholder="gpt-5.5"
                  className="mt-1"
                />
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
              onClick={() => invoke('open_external_url', { url: 'https://github.com/Zackriya-Solutions/meetily/blob/main/docs/auth/codex-auth.md' })}
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

        {modelConfig.provider === 'openclaw' && (
          <div className="space-y-4 border-t pt-4">
            <div className="flex items-start justify-between gap-3">
              <div className="flex items-start gap-3">
                <div className={cn(
                  'mt-0.5 rounded-md p-2',
                  openClawStatus?.ready ? 'bg-green-100 text-green-700' : 'bg-amber-100 text-amber-700'
                )}>
                  <ServerCog className="h-4 w-4" />
                </div>
                <div className="space-y-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="font-medium">OpenClaw gateway</span>
                    <span className={cn(
                      'rounded-full px-2 py-0.5 text-xs font-medium',
                      openClawStatus?.ready ? 'bg-green-100 text-green-800' : 'bg-amber-100 text-amber-800'
                    )}>
                      {isLoadingOpenClawStatus
                        ? 'Checking'
                        : openClawStatus?.ready
                          ? 'Ready'
                          : 'Needs setup'}
                    </span>
                  </div>
                  <p className="text-sm text-muted-foreground">
                    Route summaries through the OpenClaw ingest service. OAuth stays on the OpenClaw side; ClawScribe only stores this endpoint configuration and bearer token.
                  </p>
                  <p className="text-xs text-muted-foreground">
                    {openClawStatusError
                      ? `OpenClaw status unavailable: ${openClawStatusError}`
                      : openClawStatus?.status_message || 'OpenClaw status has not been checked yet.'}
                  </p>
                </div>
              </div>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                onClick={loadOpenClawStatus}
                disabled={isLoadingOpenClawStatus}
                title="Refresh OpenClaw status"
              >
                <RefreshCw className={cn('h-4 w-4', isLoadingOpenClawStatus && 'animate-spin')} />
              </Button>
            </div>

            <div className="flex items-center justify-between rounded-md border p-3">
              <div>
                <Label htmlFor="openclaw-enabled">Enable OpenClaw handoff</Label>
                <p className="mt-1 text-xs text-muted-foreground">
                  Submit completed recordings and allow OpenClaw-backed summaries.
                </p>
              </div>
              <Switch
                id="openclaw-enabled"
                checked={openClawEnabled}
                onCheckedChange={setOpenClawEnabled}
              />
            </div>

            <div>
              <Label htmlFor="openclaw-endpoint">Meeting Handoff URL *</Label>
              <Input
                id="openclaw-endpoint"
                type="url"
                value={openClawEndpoint}
                onChange={(e) => setOpenClawEndpoint(e.target.value)}
                placeholder="http://openclaw-host.local:8765/meetings/completed"
                className="mt-1"
              />
            </div>

            <div>
              <Label htmlFor="openclaw-model-endpoint">Summary Gateway URL *</Label>
              <Input
                id="openclaw-model-endpoint"
                type="url"
                value={openClawModelEndpoint}
                onChange={(e) => setOpenClawModelEndpoint(e.target.value)}
                placeholder="http://openclaw-host.local:8765/v1/chat/completions"
                className="mt-1"
              />
            </div>

            <div>
              <Label htmlFor="openclaw-bearer-token">Bearer Token *</Label>
              <Input
                id="openclaw-bearer-token"
                type="password"
                value={openClawBearerToken}
                onChange={(e) => setOpenClawBearerToken(e.target.value)}
                placeholder={openClawStatus?.bearer_token_configured ? 'Token already saved; enter a new one to replace it' : 'Paste the OpenClaw ingest bearer token'}
                className="mt-1"
              />
              <p className="mt-1 text-xs text-muted-foreground">
                {openClawStatus?.bearer_token_configured
                  ? 'A token is already configured. Leaving this blank keeps the saved token.'
                  : 'This must match MEETING_OPENCLAW_INGEST_TOKEN on the OpenClaw ingest service.'}
              </p>
            </div>

            <div>
              <Label htmlFor="openclaw-source">Source Name *</Label>
              <Input
                id="openclaw-source"
                value={openClawSource}
                onChange={(e) => setOpenClawSource(e.target.value)}
                placeholder="ClawScribe"
                className="mt-1"
              />
            </div>

            <div className="flex items-center justify-between rounded-md border p-3">
              <div>
                <Label htmlFor="openclaw-include-audio-path">Include local audio path</Label>
                <p className="mt-1 text-xs text-muted-foreground">
                  Include the recorder machine's audio path in submitted metadata.
                </p>
              </div>
              <Switch
                id="openclaw-include-audio-path"
                checked={openClawIncludeAudioPath}
                onCheckedChange={setOpenClawIncludeAudioPath}
              />
            </div>
          </div>
        )}

        {requiresApiKey && (
          <div>
            <Label>API Key</Label>
            <div className="relative mt-1">
              <Input
                type={showApiKey ? 'text' : 'password'}
                value={apiKey || ''}
                onChange={(e) => setApiKey(e.target.value)}
                disabled={isApiKeyLocked}
                placeholder="Enter your API key"
                className="pr-24"
              />
              {isApiKeyLocked && apiKey?.trim() && (
                <div
                  onClick={handleInputClick}
                  className="absolute inset-0 flex items-center justify-center bg-muted/50 rounded-md cursor-not-allowed"
                />
              )}
              <div className="absolute inset-y-0 right-0 pr-1 flex items-center space-x-1">
                {apiKey?.trim() && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    onClick={() => setIsApiKeyLocked(!isApiKeyLocked)}
                    className={isLockButtonVibrating ? 'animate-vibrate text-red-500' : ''}
                    title={isApiKeyLocked ? 'Unlock to edit' : 'Lock to prevent editing'}
                  >
                    {isApiKeyLocked ? <Lock /> : <Unlock />}
                  </Button>
                )}
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  onClick={() => setShowApiKey(!showApiKey)}
                >
                  {showApiKey ? <EyeOff /> : <Eye />}
                </Button>
              </div>
            </div>
          </div>
        )}

        {modelConfig.provider === 'openai' && (
          <Alert className="border-blue-200 bg-blue-50">
            <AlertDescription className="text-blue-900">
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
                  onClick={() => invoke('open_external_url', { url: 'https://platform.openai.com/api-keys' })}
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
              <Label className="cursor-pointer">Custom Endpoint (optional)</Label>
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
                        endpointValidationState === 'invalid' && "border-red-500"
                      )}
                    />
                    {endpointValidationState === 'valid' && (
                      <CheckCircle2 className="absolute right-3 top-1/2 -translate-y-1/2 h-5 w-5 text-green-500" />
                    )}
                    {endpointValidationState === 'invalid' && (
                      <XCircle className="absolute right-3 top-1/2 -translate-y-1/2 h-5 w-5 text-red-500" />
                    )}
                  </div>
                  <Button
                    type="button"
                    size={'sm'}
                    onClick={() => fetchOllamaModels()}
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
                        Fetch Models
                      </>
                    )}
                  </Button>
                </div>
                {ollamaEndpointChanged && !error && (
                  <Alert className="mt-3 border-yellow-500 bg-yellow-50">
                    <AlertDescription className="text-yellow-800">
                      Endpoint changed. Please click "Fetch Models" to load models from the new endpoint before saving.
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
              <h4 className="text-sm font-bold">Available Ollama Models</h4>
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
                    <Alert className="border-orange-500 bg-orange-50">
                      <AlertDescription className="text-orange-800">
                        Ollama is not installed or not running. Please download and install Ollama to use local models.
                      </AlertDescription>
                    </Alert>
                    <Button
                      variant="default"
                      size="sm"
                      onClick={() => invoke('open_external_url', { url: 'https://ollama.com/download' })}
                      className="w-full bg-blue-600 hover:bg-blue-700"
                    >
                      <ExternalLink className="mr-2 h-4 w-4" />
                      Download Ollama
                    </Button>
                    <div className="text-sm text-muted-foreground text-center">
                      After installing Ollama, restart this application and click "Fetch Models" to continue.
                    </div>
                  </div>
                ) : (
                  /* Show model download option when Ollama is installed but no models */
                  <>
                    <Alert className="mb-4">
                      <AlertDescription>
                        {ollamaEndpointChanged
                          ? 'Endpoint changed. Click "Fetch Models" to load models from the new endpoint.'
                          : 'No models found. Download a recommended model or click "Fetch Models" to load available Ollama models.'}
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
                              Download gemma3:1b (Recommended, ~800MB)
                            </>
                          )}
                        </Button>

                        {/* Show progress for gemma3:1b download */}
                        {isDownloading('gemma3:1b') && getProgress('gemma3:1b') !== undefined && (
                          <div className="bg-white rounded-md border p-3">
                            <div className="flex items-center justify-between mb-2">
                              <span className="text-sm font-medium text-blue-600">Downloading gemma3:1b</span>
                              <span className="text-sm font-semibold text-blue-600">
                                {Math.round(getProgress('gemma3:1b')!)}%
                              </span>
                            </div>
                            <div className="w-full h-2 bg-gray-200 rounded-full overflow-hidden">
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
                      No models found matching "{searchQuery}". Try a different search term.
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
                              ? 'ring-1 ring-blue-500 border-blue-500 background-blue-100'
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
                            <div className="mt-3 pt-3 border-t border-gray-200">
                              <div className="flex items-center justify-between mb-2">
                                <span className="text-sm font-medium text-blue-600">Downloading...</span>
                                <span className="text-sm font-semibold text-blue-600">{Math.round(progress)}%</span>
                              </div>
                              <div className="w-full h-2 bg-gray-200 rounded-full overflow-hidden">
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

      {/* Auto-generate summaries toggle */}
      {/* <div className="mt-6 pt-6 border-t border-gray-200">
        <div className="flex items-center justify-between">
          <div className="flex-1">
            <Label htmlFor="auto-generate" className="text-base font-medium">
              Auto-generate summaries
            </Label>
            <p className="text-sm text-muted-foreground mt-1">
              Automatically generate summary when opening meetings without one
            </p>
          </div>
          <Switch
            id="auto-generate"
            checked={autoGenerateEnabled}
            onCheckedChange={setAutoGenerateEnabled}
          />
        </div>
      </div> */}

      <div className="mt-6 flex justify-end">
        <Button
          className={cn(
            'px-4 text-sm font-medium text-white rounded-md focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-blue-500',
            isDoneDisabled ? 'bg-gray-400 cursor-not-allowed' : 'bg-blue-600 hover:bg-blue-700'
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
