'use client';

import { UNCONFIGURED_SUMMARY_MODEL } from '@/constants/modelDefaults';
import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { ModelConfig, ModelSettingsModal } from '@/components/ModelSettingsModal';
import { SettingsCard } from '@/components/ui/settings-layout';
import { readSecretStatus, withoutSecrets } from '@/lib/secret-key';
import type { CustomOpenAIConfig } from '@/services/configService';

interface SummaryModelSettingsProps {
  refetchTrigger?: number; // Change this to trigger refetch
}

export function SummaryModelSettings({ refetchTrigger }: SummaryModelSettingsProps) {
  const [modelConfig, setModelConfig] = useState<ModelConfig>(UNCONFIGURED_SUMMARY_MODEL);

  // Reusable fetch function
  const fetchModelConfig = useCallback(async () => {
    try {
      const raw = await invoke<ModelConfig | null>('api_get_model_config');
      // The core reports whether a key is stored, never the key.
      const data = raw && { ...withoutSecrets(raw), apiKeyStatus: readSecretStatus(raw.apiKeyStatus) };
      if (data && data.provider !== null) {
        // Fetch Custom OpenAI config if that's the active provider
        if (data.provider === 'custom-openai') {
          try {
            const customConfig = await invoke<CustomOpenAIConfig | null>('api_get_custom_openai_config');
            if (customConfig) {
              data.customOpenAIEndpoint = customConfig.endpoint || null;
              data.customOpenAIModel = customConfig.model || null;
              data.customOpenAIKeyStatus = readSecretStatus(customConfig.apiKeyStatus);
              data.maxTokens = customConfig.maxTokens || null;
              data.contextWindow = customConfig.contextWindow || null;
              data.temperature = customConfig.temperature || null;
              data.topP = customConfig.topP || null;
              data.timeoutSeconds = customConfig.timeoutSeconds || null;
              data.organization = customConfig.organization || null;
              data.project = customConfig.project || null;
              // For custom-openai, model field should match customOpenAIModel
              data.model = customConfig.model || data.model;
            }
          } catch (err) {
            console.error('Failed to fetch custom OpenAI config:', err);
          }
        }
        setModelConfig(data);
      }
    } catch (error) {
      console.error('Failed to fetch model config:', error);
      toast.error('Failed to load model settings');
    }
  }, []);

  // Fetch on mount
  useEffect(() => {
    fetchModelConfig();
  }, [fetchModelConfig]);

  // Refetch when trigger changes (optional external control)
  useEffect(() => {
    if (refetchTrigger !== undefined && refetchTrigger > 0) {
      fetchModelConfig();
    }
  }, [refetchTrigger, fetchModelConfig]);

  // Listen for model config updates from other components
  useEffect(() => {
    const setupListener = async () => {
      const { listen } = await import('@tauri-apps/api/event');
      const unlisten = await listen<ModelConfig>('model-config-updated', (event) => {
        setModelConfig(event.payload);
      });

      return unlisten;
    };

    const subscription = setupListener();
    return () => { void subscription.then(unlisten => unlisten()); };
  }, []);

  // Save handler. The Model card saves as the user changes a setting, so a
  // failure is rethrown for the field to show instead of announced in a toast.
  const handleSaveModelConfig = async (config: ModelConfig) => {
    try {
      await invoke('api_save_model_config', {
        provider: config.provider,
        model: config.model,
        whisperModel: config.whisperModel,
        apiKey: config.apiKey,
        ollamaEndpoint: config.ollamaEndpoint,
      });

      // The key (if one was typed) is stored now; keep and share only the rest.
      const shared = withoutSecrets({ ...config, customOpenAIApiKey: null });
      setModelConfig(shared);

      // Emit event to sync other components
      const { emit } = await import('@tauri-apps/api/event');
      await emit('model-config-updated', shared);
    } catch (error) {
      console.error('Error saving model config');
      throw error;
    }
  };

  return (
    <SettingsCard title="Model">
      <ModelSettingsModal
        modelConfig={modelConfig}
        setModelConfig={setModelConfig}
        onSave={handleSaveModelConfig}
        skipInitialFetch={true}
        autosave
      />
    </SettingsCard>
  );
}
