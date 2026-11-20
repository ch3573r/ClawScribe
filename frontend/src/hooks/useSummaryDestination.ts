import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { ModelConfig } from '@/services/configService';

export interface SummaryDestination {
  provider: string;
  model: string;
  endpoint: string | null;
  external: boolean;
  unencrypted: boolean;
  fingerprint: string;
  approved: boolean;
}

export type ImportSummaryChoice = 'default' | 'off' | { fingerprint: string };

export function importSummarySelection(
  choice: ImportSummaryChoice,
  automaticEnabled: boolean,
  destination: SummaryDestination | null,
) {
  if (!destination || choice === 'off') return null;
  if (choice === 'default') {
    return automaticEnabled && destination.approved
      ? { fingerprint: destination.fingerprint, useAutomaticPreference: true }
      : null;
  }
  return choice.fingerprint === destination.fingerprint
    ? { fingerprint: choice.fingerprint, useAutomaticPreference: false }
    : null;
}

export function useSummaryDestination(active: boolean, modelConfig: ModelConfig, enabled?: boolean) {
  const [destination, setDestination] = useState<SummaryDestination | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);

  useEffect(() => {
    if (!active) return;
    let current = true;
    const listener = listen('summary-destination-changed', () => {
      if (current) setRevision(value => value + 1);
    }).catch(() => undefined);
    return () => {
      current = false;
      void listener.then(unlisten => unlisten?.());
    };
  }, [active]);

  useEffect(() => {
    setDestination(null);
    setError(null);
    if (!active) return;

    let current = true;
    void invoke<SummaryDestination>('get_automatic_summary_destination')
      .then(value => { if (current) setDestination(value); })
      .catch((reason: unknown) => {
        if (current) setError(typeof reason === 'string' ? reason : 'Could not read the summary destination.');
      });
    return () => { current = false; };
  }, [active, modelConfig, enabled, revision]);

  return { destination, error, refresh: () => setRevision(value => value + 1) };
}
