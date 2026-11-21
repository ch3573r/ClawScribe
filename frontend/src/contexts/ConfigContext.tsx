'use client';

import React, { createContext, useContext, useState, useEffect, useCallback, useMemo, ReactNode, useRef } from 'react';
import { TranscriptModelProps } from '@/components/TranscriptSettings';
import {
  selectedDevicesFromPreferences,
  type SelectedDevices,
} from '@/lib/audioDevicePreferences';
import { configService, ModelConfig } from '@/services/configService';
import { invoke } from '@tauri-apps/api/core';
import { NO_SECRET, withRefreshedKeyStatus, withoutSecrets, type SecretStatus } from '@/lib/secret-key';
import { loadBehaviorSettings, mirrorLegacyBehavior, refreshBehaviorSettings } from '@/lib/behaviorSettings';
import { toast } from 'sonner';
import { useAudioDeviceInventory } from '@/hooks/useAudioDeviceInventory';

export interface OllamaModel {
  name: string;
  id: string;
  size: string;
  modified: string;
}

export type KeyProvider = 'claude' | 'groq' | 'openai' | 'openrouter';
const KEY_PROVIDERS: KeyProvider[] = ['claude', 'groq', 'openai', 'openrouter'];

export interface StorageLocations {
  database: string;
  models: string;
  recordings: string;
}

export interface NotificationSettings {
  recording_notifications: boolean;
  time_based_reminders: boolean;
  meeting_reminders: boolean;
  respect_do_not_disturb: boolean;
  notification_sound: boolean;
  system_permission_granted: boolean;
  consent_given: boolean;
  manual_dnd_mode: boolean;
  notification_preferences: {
    show_recording_started: boolean;
    show_recording_stopped: boolean;
    show_recording_paused: boolean;
    show_recording_resumed: boolean;
    show_transcription_complete: boolean;
    show_meeting_reminders: boolean;
    show_system_errors: boolean;
    meeting_reminder_minutes: number[];
  };
}

interface ConfigContextType {
  // Model configuration
  modelConfig: ModelConfig;
  setModelConfig: (config: ModelConfig | ((prev: ModelConfig) => ModelConfig)) => void;

  // Transcript model configuration
  transcriptModelConfig: TranscriptModelProps;
  setTranscriptModelConfig: (config: TranscriptModelProps | ((prev: TranscriptModelProps) => TranscriptModelProps)) => void;

  // Device configuration
  selectedDevices: SelectedDevices;
  setSelectedDevices: (devices: SelectedDevices) => void;
  audioDeviceInventory: ReturnType<typeof useAudioDeviceInventory>;
  devicePreferencesStatus: 'loading' | 'ready' | 'error';
  refreshDevicePreferences: () => Promise<void>;

  // Language preference
  selectedLanguage: string;
  setSelectedLanguage: (lang: string) => void;

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

  // Preference settings (lazy loaded)
  notificationSettings: NotificationSettings | null;
  storageLocations: StorageLocations | null;
  isLoadingPreferences: boolean;
  loadPreferences: () => Promise<void>;
  updateNotificationSettings: (settings: NotificationSettings) => Promise<void>;
}

const ConfigContext = createContext<ConfigContextType | undefined>(undefined);


export function ConfigProvider({ children }: { children: ReactNode }) {
  // Model configuration state
  const [modelConfig, setModelConfig] = useState<ModelConfig>({
    provider: 'custom-openai',
    model: 'gpt-4o-mini',
    whisperModel: 'large-v3',
    ollamaEndpoint: null,
    customOpenAIEndpoint: 'https://api.openai.com/v1',
    customOpenAIModel: 'gpt-4o-mini',
  });

  // Transcript model configuration state
  const [transcriptModelConfig, setTranscriptModelConfig] = useState<TranscriptModelProps>({
    provider: 'parakeet',
    model: 'parakeet-tdt-0.6b-v3-int8',
    apiKey: null
  });

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

  // Device configuration state
  const [selectedDevices, updateSelectedDevices] = useState<SelectedDevices>({
    micDevice: null,
    systemDevice: null
  });
  const [devicePreferencesStatus, setDevicePreferencesStatus] = useState<'loading' | 'ready' | 'error'>('loading');
  const devicePreferencesRevision = useRef(0);
  const audioDeviceInventory = useAudioDeviceInventory();
  const setSelectedDevices = useCallback((devices: SelectedDevices) => {
    devicePreferencesRevision.current += 1;
    updateSelectedDevices(devices);
    setDevicePreferencesStatus('ready');
  }, []);

  // Language preference state
  // The core owns the language: it applies it at start-up and the value shown
  // here is read back from it (`loadBehaviorSettings`), never from localStorage.
  const [selectedLanguage, setSelectedLanguage] = useState<string>('auto');

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

  // Preference settings state (lazy loaded)
  const [notificationSettings, setNotificationSettings] = useState<NotificationSettings | null>(null);
  const [storageLocations, setStorageLocations] = useState<StorageLocations | null>(null);
  const [isLoadingPreferences, setIsLoadingPreferences] = useState(false);
  const preferencesLoadedRef = useRef(false);
  const isLoadingRef = useRef(false);

  // Load Ollama models only when Ollama is selected. Other providers should not
  // probe a local Ollama service during startup.
  useEffect(() => {
    const loadModels = async () => {
      if (modelConfig.provider !== 'ollama') {
        setModels([]);
        setError('');
        return;
      }

      try {
        const endpoint = modelConfig.ollamaEndpoint || null;
        const modelList = await invoke<OllamaModel[]>('get_ollama_models', { endpoint });
        setModels(modelList);
        setError('');
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Failed to load Ollama models');
        console.error('Error loading models:', err);
      }
    };
    loadModels();
  }, [modelConfig.provider, modelConfig.ollamaEndpoint]);

  // Load transcript configuration on mount
  useEffect(() => {
    const loadTranscriptConfig = async () => {
      try {
        const config = await configService.getTranscriptConfig();
        if (config) {
          console.log('[ConfigContext] Loaded saved transcript config');
          setTranscriptModelConfig({
            // A saved config with a blank provider runs Whisper in the core (the
            // Settings page reads it the same way); only no saved config means Parakeet.
            provider: config.provider || 'localWhisper',
            model: config.model || (config.provider ? 'parakeet-tdt-0.6b-v3-int8' : 'large-v3'),
            baseUrl: config.baseUrl || null,
            allowUnencrypted: config.allowUnencrypted ?? false,
            endpoint: config.endpoint || null,
            region: config.region || null,
          });
        }
      } catch (error) {
        console.error('[ConfigContext] Failed to load transcript config:', error);
      }
    };
    loadTranscriptConfig();
  }, []);

  // Read the language from the core. The first time, the old webview value is
  // handed to the core (which keeps its own if it has one).
  useEffect(() => {
    let active = true;
    loadBehaviorSettings()
      .then(settings => { if (active) setSelectedLanguage(settings.language); })
      .catch(err => console.error('[ConfigContext] Failed to read the language from the core:', err));
    return () => { active = false; };
  }, []);

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
              if (customConfig) {
                // Merge custom config fields into modelConfig
                console.log('[ConfigContext] Loading custom OpenAI config:', {
                  endpoint: customConfig.endpoint,
                  model: customConfig.model,
                });
                const resolvedModel = customConfig.model || data.model || '';
                setModelConfig(prev => ({
                  ...prev,
                  provider: data.provider,
                  model: resolvedModel || prev.model,
                  whisperModel: data.whisperModel || prev.whisperModel,
                  customOpenAIEndpoint: customConfig.endpoint,
                  customOpenAIModel: customConfig.model,
                  customOpenAIKeyStatus: customConfig.apiKeyStatus,
                  maxTokens: customConfig.maxTokens,
                  contextWindow: customConfig.contextWindow,
                  temperature: customConfig.temperature,
                  topP: customConfig.topP,
                }));

                // Seed per-provider model cache from DB
                if (resolvedModel) {
                  const map = JSON.parse(localStorage.getItem('providerModelMap') || '{}');
                  map[data.provider] = resolvedModel;
                  localStorage.setItem('providerModelMap', JSON.stringify(map));
                }

                return; // Early return
              }
            } catch (err) {
              console.error('[ConfigContext] Failed to fetch custom OpenAI config:', err);
            }
          }

          // For non-custom-openai providers, just set base config
          setModelConfig(prev => ({
            ...prev,
            provider: data.provider,
            model: data.model || prev.model,
            whisperModel: data.whisperModel || prev.whisperModel,
            ollamaEndpoint: data.ollamaEndpoint,
            apiKeyStatus: data.apiKeyStatus,
          }));

          // Seed per-provider model cache from DB
          if (data.model) {
            const map = JSON.parse(localStorage.getItem('providerModelMap') || '{}');
            map[data.provider] = data.model;
            localStorage.setItem('providerModelMap', JSON.stringify(map));
          }
        }
      } catch (error) {
        console.error('Failed to fetch saved model config in ConfigContext:', error);
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
        console.error('[ConfigContext] Failed to load provider key status');
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
      console.error('[ConfigContext] Failed to read provider key status');
    }
  }, []);

  // Listen for model config updates from other components
  useEffect(() => {
    const setupListener = async () => {
      const { listen } = await import('@tauri-apps/api/event');
      const unlisten = await listen<ModelConfig>('model-config-updated', (event) => {
        console.log('[ConfigContext] Received model-config-updated event');
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

  // Load device preferences on mount
  const refreshDevicePreferences = useCallback(async () => {
    const request = ++devicePreferencesRevision.current;
    setDevicePreferencesStatus('loading');
    try {
      const prefs = await configService.getRecordingPreferences();
      if (request === devicePreferencesRevision.current) setSelectedDevices(selectedDevicesFromPreferences(prefs));
    } catch {
      if (request === devicePreferencesRevision.current) setDevicePreferencesStatus('error');
    }
  }, [setSelectedDevices]);
  useEffect(() => {
    void refreshDevicePreferences();
    return () => { devicePreferencesRevision.current += 1; };
  }, [refreshDevicePreferences]);

  // Calculate model options based on available models
  const modelOptions = useMemo<Record<ModelConfig['provider'], string[]>>(() => ({
    ollama: models.map(model => model.name),
    claude: ['claude-opus-5'],
    groq: ['llama-3.3-70b-versatile'],
    openrouter: [],
    openai: ['gpt-6.1-sol', 'gpt-6-sol', 'gpt-6-luna', 'gpt-4o-mini'],
    'builtin-ai': [],
    'custom-openai': [modelConfig.customOpenAIModel || modelConfig.model || 'gpt-4o-mini'],
    openclaw: ['openclaw-managed'],
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

  // Lazy load preference settings (only loads if not already cached)
  const loadPreferences = useCallback(async () => {
    // If already loaded, don't reload
    if (preferencesLoadedRef.current) {
      return;
    }

    // If currently loading, don't start another load
    if (isLoadingRef.current) {
      return;
    }

    isLoadingRef.current = true;
    setIsLoadingPreferences(true);
    try {
      // Load notification settings from backend
      let settings: NotificationSettings | null = null;
      try {
        settings = await invoke<NotificationSettings>('get_notification_settings');
        setNotificationSettings(settings);
      } catch (notifError) {
        console.error('[ConfigContext] Failed to load notification settings:', notifError);
        // Use default values if notification settings fail to load
        setNotificationSettings(null);
      }

      // Load storage locations
      const [dbDir, modelsDir, recordingsDir] = await Promise.all([
        invoke<string>('get_database_directory'),
        invoke<string>('whisper_get_models_directory'),
        invoke<string>('get_default_recordings_folder_path')
      ]);

      setStorageLocations({
        database: dbDir,
        models: modelsDir,
        recordings: recordingsDir
      });

      // Mark as loaded
      preferencesLoadedRef.current = true;
    } catch (error) {
      console.error('[ConfigContext] Failed to load preferences:', error);
    } finally {
      isLoadingRef.current = false;
      setIsLoadingPreferences(false);
    }
  }, []);

  // Update notification settings
  const updateNotificationSettings = useCallback(async (settings: NotificationSettings) => {
    try {
      await invoke('set_notification_settings', { settings });
      setNotificationSettings(settings);
    } catch (error) {
      console.error('[ConfigContext] Failed to update notification settings:', error);
      throw error; // Re-throw so component can handle error
    }
  }, []);

  // The core stores the language first. A failed save leaves the language as
  // it was and says so; nothing is shown as changed that the core did not accept.
  const handleSetSelectedLanguage = useCallback((lang: string) => {
    invoke('set_language_preference', { language: lang })
      .then(async () => {
        // LEGACY mirror, so a downgrade to 0.62.0 sees the current value.
        mirrorLegacyBehavior('language', lang);
        const settings = await refreshBehaviorSettings();
        setSelectedLanguage(settings.language);
      })
      .catch(err => {
        console.error('Failed to save the language in the core:', err);
        toast.error('Could not save the language');
      });
  }, []);

  const value: ConfigContextType = useMemo(() => ({
    modelConfig,
    setModelConfig,
    isAutoSummary,
    automaticSummaryBusy,
    toggleIsAutoSummary,
    providerKeyStatus,
    refreshProviderKeyStatus,
    transcriptModelConfig,
    setTranscriptModelConfig,
    selectedDevices,
    setSelectedDevices,
    audioDeviceInventory,
    devicePreferencesStatus,
    refreshDevicePreferences,
    selectedLanguage,
    setSelectedLanguage: handleSetSelectedLanguage,
    models,
    modelOptions,
    error,
    notificationSettings,
    storageLocations,
    isLoadingPreferences,
    loadPreferences,
    updateNotificationSettings,
  }), [
    modelConfig,
    isAutoSummary,
    automaticSummaryBusy,
    toggleIsAutoSummary,
    providerKeyStatus,
    refreshProviderKeyStatus,
    transcriptModelConfig,
    selectedDevices,
    setSelectedDevices,
    audioDeviceInventory,
    devicePreferencesStatus,
    refreshDevicePreferences,
    selectedLanguage,
    handleSetSelectedLanguage,
    models,
    modelOptions,
    error,
    notificationSettings,
    storageLocations,
    isLoadingPreferences,
    loadPreferences,
    updateNotificationSettings,
  ]);

  return (
    <ConfigContext.Provider value={value}>
      {children}
    </ConfigContext.Provider>
  );
}

export function useConfig() {
  const context = useContext(ConfigContext);
  if (context === undefined) {
    throw new Error('useConfig must be used within a ConfigProvider');
  }
  return context;
}
