import { useState, useCallback, useRef, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useCloudTranscription } from '@/hooks/useCloudTranscription';

export interface RawModelInfo {
  name: string;
  size_mb: number;
  status: 'Available' | 'Missing' | { Downloading: { progress: number } } | { Error: string };
}

export interface ModelOption {
  provider: 'whisper' | 'parakeet' | 'nemotron' | 'cloud-whisper' | 'mai-transcribe';
  name: string;
  displayName: string;
  size_mb: number;
}

interface TranscriptModelConfig {
  provider?: string;
  model?: string;
}

/**
 * Custom hook for fetching and managing transcription models (Whisper and Parakeet).
 *
 * This hook centralizes the model fetching logic that was previously duplicated
 * in ImportAudioDialog and RetranscribeDialog components.
 *
 * @param transcriptModelConfig - User's saved model configuration from context
 * @returns Object containing available models, selected model key, loading state, and fetch function
 */
export function useTranscriptionModels(transcriptModelConfig: TranscriptModelConfig | undefined) {
  const [localModels, setLocalModels] = useState<ModelOption[]>([]);
  const [manualSelection, setManualSelection] = useState('');
  const [loadingModels, setLoadingModels] = useState(false);
  const { enabled, loaded } = useCloudTranscription();
  const revision = useRef(0);
  const fetchModels = useCallback(async () => {
    const request = ++revision.current;
    setLoadingModels(true);
    const allModels: ModelOption[] = [];

    // Fetch Whisper models
    try {
      const whisperModels = await invoke<RawModelInfo[]>('whisper_get_available_models');
      const availableWhisper = whisperModels
        .filter((m) => m.status === 'Available')
        .map((m) => ({
          provider: 'whisper' as const,
          name: m.name,
          displayName: `🏠 Whisper: ${m.name}`,
          size_mb: m.size_mb,
        }));
      allModels.push(...availableWhisper);
    } catch (err) {
      console.error('Failed to fetch Whisper models:', err);
    }

    // Fetch Parakeet models
    try {
      const parakeetModels = await invoke<RawModelInfo[]>('parakeet_get_available_models');
      const availableParakeet = parakeetModels
        .filter((m) => m.status === 'Available')
        .map((m) => ({
          provider: 'parakeet' as const,
          name: m.name,
          displayName: `⚡ Parakeet: ${m.name}`,
          size_mb: m.size_mb,
        }));
      allModels.push(...availableParakeet);
    } catch (err) {
      console.error('Failed to fetch Parakeet models:', err);
    }

    // Fetch Nemotron models (Beta)
    try {
      const nemotronModels = await invoke<RawModelInfo[]>('nemotron_get_available_models');
      const availableNemotron = nemotronModels
        .filter((m) => m.status === 'Available')
        .map((m) => ({
          provider: 'nemotron' as const,
          name: m.name,
          displayName: `🌊 Nemotron: ${m.name}`,
          size_mb: m.size_mb,
        }));
      allModels.push(...availableNemotron);
    } catch (err) {
      console.error('Failed to fetch Nemotron models:', err);
    }

    if (request === revision.current) {
      setLocalModels(allModels);
      setLoadingModels(false);
    }
  }, []);

  const provider = transcriptModelConfig?.provider;
  const model = transcriptModelConfig?.model;
  const availableModels = useMemo(() => {
    if (!loaded || !enabled) return localModels;
    const hostedWhisper = provider === 'cloud-whisper' && model ? model : 'whisper-1';
    return [
      ...localModels,
      { provider: 'cloud-whisper' as const, name: hostedWhisper, displayName: `Hosted Whisper · ${hostedWhisper}`, size_mb: 0 },
      // Keep the legacy provider id so saved endpoints and credentials survive.
      { provider: 'mai-transcribe' as const, name: 'azure-speech', displayName: 'Azure Speech', size_mb: 0 },
    ];
  }, [localModels, loaded, enabled, provider, model]);

  const keyOf = (option: ModelOption) => `${option.provider}:${option.name}`;
  const preferred = availableModels.find(option =>
    option.provider === (provider === 'localWhisper' ? 'whisper' : provider)
      && (provider === 'mai-transcribe' || option.name === model));
  const selected = availableModels.find(option => keyOf(option) === manualSelection) ?? preferred ?? localModels[0];
  const selectedModelKey = selected ? keyOf(selected) : '';
  const resetSelection = useCallback(() => setManualSelection(''), []);
  return {
    availableModels,
    selectedModelKey,
    setSelectedModelKey: setManualSelection,
    loadingModels,
    fetchModels,
    resetSelection,
  };
}
