'use client';

import { useMemo, type ReactNode } from 'react';
import { SummaryConfigProvider, useSummaryConfig } from './SummaryConfigContext';
import { TranscriptionConfigProvider, useTranscriptionConfig } from './TranscriptionConfigContext';
import { PreferenceConfigProvider, usePreferenceConfig } from './PreferenceConfigContext';

export type { KeyProvider, OllamaModel } from './SummaryConfigContext';
export type { NotificationSettings, StorageLocations } from './PreferenceConfigContext';
export { useSummaryConfig, useTranscriptionConfig, usePreferenceConfig };

export function ConfigProvider({ children }: { children: ReactNode }) {
  return (
    <SummaryConfigProvider>
      <TranscriptionConfigProvider>
        <PreferenceConfigProvider>{children}</PreferenceConfigProvider>
      </TranscriptionConfigProvider>
    </SummaryConfigProvider>
  );
}

/** Compatibility for callers that read more than one domain. */
export function useConfig() {
  const summary = useSummaryConfig();
  const transcription = useTranscriptionConfig();
  const preferences = usePreferenceConfig();
  return useMemo(() => ({ ...summary, ...transcription, ...preferences }), [summary, transcription, preferences]);
}
