import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { LiveModelFailure } from '@/lib/start-error';

interface LiveTranscriptionReadiness {
  provider: 'parakeet' | 'localWhisper' | 'nemotron' | null;
  hostedBatchProvider: 'cloud-whisper' | 'mai-transcribe' | null;
  modelIssue: LiveModelFailure | null;
}

export function useLiveTranscriptionReadiness(inspectModel: boolean, configuredProvider: string, configuredModel: string) {
  const scope = JSON.stringify([inspectModel, configuredProvider, configuredModel]);
  const [result, setResult] = useState<{ scope: string; data: LiveTranscriptionReadiness | null } | null>(null);
  const revision = useRef(0);
  const refresh = useCallback(async () => {
    const request = ++revision.current;
    setResult(null);
    try {
      const readiness = await invoke<LiveTranscriptionReadiness>('get_live_transcription_readiness', { inspectModel });
      if (request === revision.current) setResult({ scope, data: readiness });
    } catch {
      if (request === revision.current) setResult({ scope, data: null });
    }
  }, [inspectModel, scope]);

  useEffect(() => {
    void refresh();
    return () => { revision.current += 1; };
  }, [refresh]);

  useEffect(() => {
    const recheck = () => { void refresh(); };
    window.addEventListener('focus', recheck);
    const subscriptions = inspectModel ? [
      'model-download-complete', 'model-download-error',
      'parakeet-model-download-complete', 'parakeet-model-download-error',
      'nemotron-model-download-complete', 'nemotron-model-download-error',
    ].map(event => listen(event, recheck).catch(() => null)) : [];
    return () => {
      window.removeEventListener('focus', recheck);
      subscriptions.forEach(subscription => void subscription.then(unlisten => unlisten?.()));
    };
  }, [inspectModel, refresh]);

  const current = result?.scope === scope ? result : null;
  return { data: current?.data ?? null, error: current !== null && current.data === null, refresh };
}
