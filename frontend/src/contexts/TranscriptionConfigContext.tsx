'use client';

import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { DEFAULT_PARAKEET_MODEL } from '@/constants/modelDefaults';
import type { TranscriptModelProps } from '@/components/TranscriptSettings';
import { configService } from '@/services/configService';
import { loadBehaviorSettings, mirrorLegacyBehavior, refreshBehaviorSettings } from '@/lib/behaviorSettings';
import { useAudioDeviceInventory } from '@/hooks/useAudioDeviceInventory';
import { useRecordingDevicePreferences } from '@/hooks/useRecordingDevicePreferences';
import { toast } from 'sonner';

interface TranscriptionConfig extends ReturnType<typeof useRecordingDevicePreferences> {
  transcriptModelConfig: TranscriptModelProps;
  setTranscriptModelConfig: (config: TranscriptModelProps | ((prev: TranscriptModelProps) => TranscriptModelProps)) => void;

  transcriptConfigStatus: 'loading' | 'ready' | 'error';
  loadTranscriptConfig: () => Promise<void>;

  // Device configuration
  audioDeviceInventory: ReturnType<typeof useAudioDeviceInventory>;

  // Language preference
  selectedLanguage: string;
  setSelectedLanguage: (lang: string) => void;
}

const TranscriptionConfigContext = createContext<TranscriptionConfig | undefined>(undefined);

export function TranscriptionConfigProvider({ children }: { children: ReactNode }) {
  // Transcript model configuration state
  const [transcriptModelConfig, setTranscriptModelConfig] = useState<TranscriptModelProps>({
    provider: 'parakeet',
    model: DEFAULT_PARAKEET_MODEL,
    apiKey: null
  });

  // Device configuration state
  const { selectedDevices, devicePreferencesStatus, isSavingDevicePreferences, refreshDevicePreferences, saveSelectedDevices } = useRecordingDevicePreferences();
  const audioDeviceInventory = useAudioDeviceInventory();

  // Language preference state
  // The core owns the language: it applies it at start-up and the value shown
  // here is read back from it (`loadBehaviorSettings`), never from localStorage.
  const [selectedLanguage, setSelectedLanguage] = useState<string>('auto');

  const [transcriptConfigStatus, setTranscriptConfigStatus] = useState<'loading' | 'ready' | 'error'>('loading');
  const loadTranscriptConfig = useCallback(async () => {
    setTranscriptConfigStatus('loading');
    try {
      const config = await configService.getTranscriptConfig();
      if (config) {
        setTranscriptModelConfig({
          // A blank provider in a saved legacy row resolves to Whisper in the core.
          provider: config.provider || 'localWhisper',
          model: config.model || '',
          baseUrl: config.baseUrl || null,
          allowUnencrypted: config.allowUnencrypted ?? false,
          endpoint: config.endpoint || null,
          region: config.region || null,
        });
      }
      setTranscriptConfigStatus('ready');
    } catch {
      setTranscriptConfigStatus('error');
    }
  }, []);
  useEffect(() => { void loadTranscriptConfig(); }, [loadTranscriptConfig]);

  // Read the language from the core. The first time, the old webview value is
  // handed to the core (which keeps its own if it has one).
  useEffect(() => {
    let active = true;
    loadBehaviorSettings()
      .then(settings => { if (active) setSelectedLanguage(settings.language); })
      .catch(err => console.error('[TranscriptionConfig] Failed to read the language from the core:', err));
    return () => { active = false; };
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

  const value = useMemo<TranscriptionConfig>(() => ({
    transcriptModelConfig,
    transcriptConfigStatus,
    loadTranscriptConfig,
    setTranscriptModelConfig,
    selectedDevices,
    saveSelectedDevices,
    isSavingDevicePreferences,
    audioDeviceInventory,
    devicePreferencesStatus,
    refreshDevicePreferences,
    selectedLanguage,
    setSelectedLanguage: handleSetSelectedLanguage,
  }), [
    transcriptModelConfig,
    transcriptConfigStatus,
    loadTranscriptConfig,
    selectedDevices,
    saveSelectedDevices,
    isSavingDevicePreferences,
    audioDeviceInventory,
    devicePreferencesStatus,
    refreshDevicePreferences,
    selectedLanguage,
    handleSetSelectedLanguage,
  ]);

  return <TranscriptionConfigContext.Provider value={value}>{children}</TranscriptionConfigContext.Provider>;
}

export function useTranscriptionConfig() {
  const value = useContext(TranscriptionConfigContext);
  if (!value) throw new Error('useTranscriptionConfig must be used within a ConfigProvider');
  return value;
}
