/**
 * Configuration Service
 *
 * Handles all configuration-related Tauri backend calls.
 * Pure 1-to-1 wrapper - no error handling changes, exact same behavior as direct invoke calls.
 */

import { invoke } from '@tauri-apps/api/core';
import { TranscriptModelProps } from '@/components/TranscriptSettings';
import { readSecretStatus, withoutSecrets, type SecretStatus } from '@/lib/secret-key';

export interface ModelConfig {
  provider: 'ollama' | 'groq' | 'claude' | 'claude-code' | 'openrouter' | 'openai' | 'builtin-ai' | 'custom-openai' | 'openclaw' | 'codex';
  model: string;
  whisperModel: string;
  /**
   * A key the user has just typed, held only until it is saved. It is never
   * loaded from the core, which does not return stored keys.
   */
  apiKey?: string | null;
  /** Whether the provider has a stored key, with a short hint. */
  apiKeyStatus?: SecretStatus;
  ollamaEndpoint?: string | null;
  // Custom OpenAI fields (only populated when provider is 'custom-openai')
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

/** The custom provider settings as the core returns them: no key, only its status. */
export interface CustomOpenAIConfig {
  allow_unencrypted?: boolean;
  destination_problem?: string | null;
  endpoint: string;
  apiKeyStatus: SecretStatus;
  model: string;
  maxTokens: number | null;
  contextWindow?: number | null;
  temperature: number | null;
  topP: number | null;
  timeoutSeconds?: number | null;
  organization?: string | null;
  project?: string | null;
}

/** What a save sends for the custom provider. A null `apiKey` keeps the stored key. */
export type CustomOpenAIConfigInput = Omit<CustomOpenAIConfig, 'apiKeyStatus' | 'destination_problem'> & {
  apiKey?: string | null;
  allowUnencrypted?: boolean;
};

export interface RecordingPreferences {
  preferred_mic_device: string | null;
  preferred_system_device: string | null;
}

/**
 * Configuration Service
 * Singleton service for managing app configuration
 */
export class ConfigService {
  /**
   * Get saved transcript model configuration
   * @returns Promise with { provider, model, apiKey }
   */
  async getTranscriptConfig(): Promise<TranscriptModelProps> {
    return invoke<TranscriptModelProps>('api_get_transcript_config');
  }

  /**
   * Get saved summary model configuration
   * @returns Promise with { provider, model, whisperModel }
   */
  async getModelConfig(): Promise<ModelConfig> {
    const raw = await invoke<ModelConfig & { apiKeyStatus?: unknown }>('api_get_model_config');
    if (!raw) return raw;
    return { ...withoutSecrets(raw), apiKeyStatus: readSecretStatus(raw.apiKeyStatus) };
  }

  /** Whether a summary provider has a stored key. The key itself is never returned. */
  async getApiKeyStatus(provider: string): Promise<SecretStatus> {
    return readSecretStatus(await invoke<unknown>('api_get_api_key_status', { provider }));
  }

  /** Removes a summary provider's stored key. */
  async clearApiKey(provider: string): Promise<void> {
    await invoke('api_clear_api_key', { provider });
  }

  /** Removes the custom provider's stored key; its other settings stay. */
  async clearCustomOpenAIApiKey(): Promise<void> {
    await invoke('api_clear_custom_openai_api_key');
  }

  /** Whether a hosted transcription provider has a stored key. The key itself is never returned. */
  async getTranscriptApiKeyStatus(provider: string): Promise<SecretStatus> {
    return readSecretStatus(await invoke<unknown>('api_get_transcript_api_key_status', { provider }));
  }

  /** Removes a hosted transcription provider's stored key. */
  async clearTranscriptApiKey(provider: string): Promise<void> {
    await invoke('api_clear_transcript_api_key', { provider });
  }

  /**
   * Get saved audio device preferences
   * @returns Promise with { preferred_mic_device, preferred_system_device }
   */
  async getRecordingPreferences(): Promise<RecordingPreferences> {
    return invoke<RecordingPreferences>('get_recording_preferences');
  }

  /**
   * Get custom OpenAI configuration
   * @returns Promise with CustomOpenAIConfig or null if not configured
   */
  async getCustomOpenAIConfig(): Promise<CustomOpenAIConfig | null> {
    const raw = await invoke<(CustomOpenAIConfig & { apiKeyStatus?: unknown }) | null>('api_get_custom_openai_config');
    if (!raw) return null;
    return { ...withoutSecrets(raw), apiKeyStatus: readSecretStatus(raw.apiKeyStatus) };
  }

  /**
   * Save custom OpenAI configuration
   * @param config - CustomOpenAIConfig to save
   * @returns Promise with result status
   */
  async saveCustomOpenAIConfig(config: CustomOpenAIConfigInput): Promise<{ status: string; message: string }> {
    return invoke<{ status: string; message: string }>('api_save_custom_openai_config', {
      endpoint: config.endpoint,
      apiKey: config.apiKey ?? null,
      model: config.model,
      maxTokens: config.maxTokens,
      temperature: config.temperature,
      topP: config.topP,
      timeoutSeconds: config.timeoutSeconds,
      organization: config.organization,
      project: config.project,
    });
  }

  /**
   * Test custom OpenAI connection
   * @param endpoint - API endpoint URL
   * @param apiKey - A key typed for this test. Null uses the stored key, for the stored endpoint only
   * @param model - Model name
   * @returns Promise with test result
   */
  async testCustomOpenAIConnection(
    endpoint: string,
    apiKey: string | null,
    model: string
  ): Promise<{ status: string; message: string; http_status?: number }> {
    return invoke<{ status: string; message: string; http_status?: number }>('api_test_custom_openai_connection', {
      endpoint,
      apiKey,
      model,
      maxTokens: null,
      temperature: null,
      topP: null,
      timeoutSeconds: null,
      organization: null,
      project: null,
    });
  }
}

// Export singleton instance
export const configService = new ConfigService();
