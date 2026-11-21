import Head from 'next/head';
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import { CALL_PROVIDERS } from '@/lib/autoRecord';
import type { CallPrompt } from '@/services/callDetectionService';
import { applyThemePreference, subscribeToSystemTheme } from '@/lib/theme';

export default function CallRecordingPrompt() {
  const [prompt, setPrompt] = useState<CallPrompt | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  const answering = useRef(false);

  useEffect(() => {
    const theme = () => applyThemePreference('system');
    theme();
    return subscribeToSystemTheme(theme);
  }, []);

  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    async function refresh() {
      try {
        const next = await invoke<CallPrompt | null>('get_pending_call_prompt');
        if (!disposed) {
          setPrompt(next);
          setError(false);
        }
      } catch {
        if (!disposed) {
          setPrompt(null);
          setError(true);
        }
      }
      if (!disposed) timer = setTimeout(refresh, 1000);
    }
    void refresh();
    return () => { disposed = true; clearTimeout(timer); };
  }, []);

  async function confirm(key: string) {
    if (answering.current) return;
    answering.current = true;
    setBusy(true);
    try {
      await invoke('confirm_call_detection_prompt', { key });
    } catch {
      setError(true);
    } finally {
      answering.current = false;
      setBusy(false);
    }
  }

  return <>
    <Head>
      <title>Record this call</title>
      <meta httpEquiv="Content-Security-Policy" content="frame-src 'none'; object-src 'none'; media-src 'none'; img-src 'self'; form-action 'none'; connect-src 'self' ipc: http://ipc.localhost https://ipc.localhost" />
    </Head>
    <main className="h-screen space-y-3 overflow-y-auto bg-background p-4 text-foreground">
      {prompt && <>
        <h1 className="font-semibold">{prompt.retry ? 'Could not start recording' : `${CALL_PROVIDERS[prompt.provider]} call detected`}</h1>
        <p className="line-clamp-2 break-words text-sm">{prompt.eventTitle ? `Is this the call for “${prompt.eventTitle}”?` : 'Start recording this call?'}</p>
        <Button disabled={busy} onClick={() => void confirm(prompt.key)}>{prompt.retry ? 'Try again' : 'Record'}</Button>
      </>}
      {error && <p role="alert" className="text-sm text-destructive">Could not check the call. You can also answer from the tray.</p>}
    </main>
  </>;
}
