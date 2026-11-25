'use client';

import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { UNCONFIGURED_SUMMARY_MODEL } from '@/constants/modelDefaults';
import { configService, customOpenAIModelFields, type ModelConfig } from '@/services/configService';
import { NO_SECRET, withRefreshedKeyStatus, withoutSecrets, type SecretStatus } from '@/lib/secret-key';
import { toast } from 'sonner';

export interface OllamaModel {
  name: string;
  id: string;
  size: string;
  modified: string;
}

export type KeyProvider = 'claude' | 'groq' | 'openai' | 'openrouter';
const KEY_PROVIDERS: KeyProvider[] = ['claude', 'groq', 'openai', 'openrouter'];

interface SummaryConfig {
  modelConfig: ModelConfig;
  setModelConfig: (config: ModelConfig | ((prev: ModelConfig) => ModelConfig)) => void;

  // Ollama models
  models: OllamaModel[];
  modelOptions: Record<ModelConfig['provider'], string[]>;
  error: string;

  // Summary configuration
  isAutoSummary: boolean;
  automaticSummaryBusy: boolean;
  toggleIsAutoSummary: (checked: boolean, fingerprint?: string) => Promise<boolean>;

  // Which providers have a stored API key. The keys themselves stay in the core.
  providerKeyStatus: Record<KeyProvider, SecretStatus>;
  refreshProviderKeyStatus: (provider: string) => Promise<void>;
}

const SummaryConfigContext = createContext<SummaryConfig | undefined>(undefined);

export function SummaryConfigProvider({ children }: { children: ReactNode }) {
  // Model configuration state
  const [modelConfig, setModelConfig] = useState<ModelConfig>(UNCONFIGURED_SUMMARY_MODEL);

  // Whether each provider has a stored key (loaded at startup). The keys stay in the core.
  const [providerKeyStatus, setProviderKeyStatus] = useState<Record<KeyProvider, SecretStatus>>({
    claude: NO_SECRET,
    groq: NO_SECRET,
    openai: NO_SECRET,
    openrouter: NO_SECRET,
  });

  // Ollama models list and error state
  const [models, setModels] = useState<OllamaModel[]>([]);
  const [error, setError] = useState<string>('');

  // Summary configs
  const [isAutoSummary, setIsAutoSummary] = useState(false);
  const [automaticSummaryBusy, setAutomaticSummaryBusy] = useState(true);
  useEffect(() => {
    let active = true;
    let legacyEnabled: boolean | null = null;
    try {
      const legacy = localStorage.getItem('isAutoSummary');
      if (legacy === 'true' || legacy === 'false') legacyEnabled = legacy === 'true';
    } catch { /* The native preference still loads without webview storage. */ }
    void invoke<boolean>('migrate_automatic_summary_preference', { legacyEnabled })
      .catch(() => invoke<boolean>('get_automatic_summary_preference'))
      .then(enabled => { if (active) setIsAutoSummary(enabled); })
      .catch(() => { if (active) toast.error('Could not read automatic summary settings'); })
      .finally(() => { if (active) setAutomaticSummaryBusy(false); });
    return () => { active = false; };
  }, []);

  // Load Ollama models only when Ollama is selected. Other providers should not
  // probe a local Ollama service during startup.
  useEffect(() => {
    let active = true;
    const loadModels = async () => {
      if (modelConfig.provider !== 'ollama') {
        setModels([]);
        setError('');
        return;
      }

      try {
        const endpoint = modelConfig.ollamaEndpoint || null;
        const modelList = await invoke<OllamaModel[]>('get_ollama_models', { endpoint });
        if (!active) return;
        setModels(modelList);
        setError('');
      } catch (err) {
        if (!active) return;
        setError(err instanceof Error ? err.message : 'Failed to load Ollama models');
        console.error('Error loading models:', err);
      }
    };
    void loadModels();
    return () => { active = false; };
  }, [modelConfig.provider, modelConfig.ollamaEndpoint]);

  // Load model configuration on mount
  useEffect(() => {
    const fetchModelConfig = async () => {
      try {
        const data = await configService.getModelConfig();
        if (data && data.provider) {
          // If provider is custom-openai, fetch the additional config
          if (data.provider === 'custom-openai') {
            try {
              const customConfig = await configService.getCustomOpenAIConfig();
              Object.assign(data, customOpenAIModelFields(customConfig));
              data.model = customConfig?.model || data.model;
            } catch (err) {
              console.error('[SummaryConfig] Failed to fetch custom OpenAI config:', err);
            }
          }

          setModelConfig(prev => ({
            ...prev,
            ...data,
            model: data.model || prev.model,
            whisperModel: data.whisperModel || prev.whisperModel,
          }));

          // Seed per-provider model cache from DB
          if (data.model) {
            const map = JSON.parse(localStorage.getItem('providerModelMap') || '{}');
            map[data.provider] = data.model;
            localStorage.setItem('providerModelMap', JSON.stringify(map));
          }
        }
      } catch (error) {
        console.error('Failed to fetch saved summary model config:', error);
      }
    };
    fetchModelConfig();
  }, []);

  // Load which providers have a stored key on mount
  useEffect(() => {
    const loadAllKeyStatus = async () => {
      try {
        const statuses = await Promise.all(
          KEY_PROVIDERS.map(p => configService.getApiKeyStatus(p).catch(() => NO_SECRET))
        );
        setProviderKeyStatus({
          claude: statuses[0],
          groq: statuses[1],
          openai: statuses[2],
          openrouter: statuses[3],
        });
      } catch {
        console.error('[SummaryConfig] Failed to load provider key status');
      }
    };

    loadAllKeyStatus();
  }, []);

  // Ask the core again whether a provider has a stored key (after a save or a removal)
  // The shared model configuration carries the status too, so a removal shows
  // at once for the provider in use, not only in the per-provider map.
  const refreshProviderKeyStatus = useCallback(async (provider: string) => {
    try {
      if (provider === 'custom-openai') {
        const config = await configService.getCustomOpenAIConfig();
        const status = config?.apiKeyStatus ?? NO_SECRET;
        setModelConfig(prev => withRefreshedKeyStatus(prev, provider, status));
        return;
      }
      if (!KEY_PROVIDERS.includes(provider as KeyProvider)) return;
      const status = await configService.getApiKeyStatus(provider);
      setProviderKeyStatus(prev => ({ ...prev, [provider]: status }));
      setModelConfig(prev => withRefreshedKeyStatus(prev, provider, status));
    } catch {
      console.error('[SummaryConfig] Failed to read provider key status');
    }
  }, []);

  // Listen for model config updates from other components
  useEffect(() => {
    const setupListener = async () => {
      const { listen } = await import('@tauri-apps/api/event');
      const unlisten = await listen<ModelConfig>('model-config-updated', (event) => {
        console.log('[SummaryConfig] Received model-config-updated event');
        // Never keep a typed key: the core has it now.
        setModelConfig(withoutSecrets(event.payload));

        // The key may have changed with this save.
        void refreshProviderKeyStatus(event.payload.provider);
      });
      return unlisten;
    };

    const subscription = setupListener();
    return () => { void subscription.then(unlisten => unlisten()); };
  }, [refreshProviderKeyStatus]);

  // Calculate model options based on available models
  const modelOptions = useMemo<Record<ModelConfig['provider'], string[]>>(() => ({
    ollama: models.map(model => model.name),
    claude: ['claude-opus-5'],
    groq: ['llama-3.3-70b-versatile'],
    openrouter: [],
    openai: ['gpt-6.1-sol', 'gpt-6-sol', 'gpt-6-luna', 'gpt-4o-mini'],
    'builtin-ai': [],
    'custom-openai': [modelConfig.customOpenAIModel || modelConfig.model || 'gpt-4o-mini'],
    codex: ['gpt-5.6-sol'],
    'claude-code': ['sonnet', 'opus', 'haiku'],
  }), [models, modelConfig.customOpenAIModel, modelConfig.model]);

  const toggleIsAutoSummary = useCallback(async (enabled: boolean, fingerprint?: string) => {
    setAutomaticSummaryBusy(true);
    try {
      await invoke('set_automatic_summary_preference', { enabled, fingerprint });
      setIsAutoSummary(enabled);
      return true;
    } catch (reason) {
      toast.error(typeof reason === 'string' ? reason : 'Could not save the automatic summary setting');
      return false;
    } finally {
      setAutomaticSummaryBusy(false);
    }
  }, []);

  const value = useMemo<SummaryConfig>(() => ({
    modelConfig,
    setModelConfig,
    models,
    modelOptions,
    error,
    isAutoSummary,
    automaticSummaryBusy,
    toggleIsAutoSummary,
    providerKeyStatus,
    refreshProviderKeyStatus,
  }), [
    modelConfig,
    models,
    modelOptions,
    error,
    isAutoSummary,
    automaticSummaryBusy,
    toggleIsAutoSummary,
    providerKeyStatus,
    refreshProviderKeyStatus,
  ]);

  return <SummaryConfigContext.Provider value={value}>{children}</SummaryConfigContext.Provider>;
}

export function useSummaryConfig() {
  const value = useContext(SummaryConfigContext);
  if (!value) throw new Error('useSummaryConfig must be used within a ConfigProvider');
  return value;
}
