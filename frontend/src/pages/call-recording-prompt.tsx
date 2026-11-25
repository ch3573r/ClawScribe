import Head from 'next/head';
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import { CALL_PROVIDERS } from '@/lib/autoRecord';
import type { CallPrompt } from '@/services/callDetectionService';
import { applyAccent, applyThemePreference, DEFAULT_ACCENT_ID, subscribeToSystemTheme, type ThemePreference } from '@/lib/theme';

interface Appearance {
  theme: ThemePreference;
  accent: string | null;
}

const CHECK_FAILED = 'Could not check the call. You can also answer from the tray.';

export default function CallRecordingPrompt() {
  const [prompt, setPrompt] = useState<CallPrompt | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [appearance, setAppearance] = useState<Appearance | null>(null);
  const answering = useRef(false);

  useEffect(() => {
    let disposed = false;
    void invoke<Appearance | null>('recording_window_appearance').then(value => {
      if (!disposed) setAppearance(value);
    }).catch(() => {});
    return () => { disposed = true; };
  }, []);

  useEffect(() => {
    const applyAppearance = () => {
      applyThemePreference(appearance?.theme ?? 'system');
      applyAccent(appearance?.accent ?? DEFAULT_ACCENT_ID);
    };
    applyAppearance();
    return subscribeToSystemTheme(applyAppearance);
  }, [appearance]);

  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    async function refresh() {
      try {
        const next = await invoke<CallPrompt | null>('get_pending_call_prompt');
        if (!disposed) {
          setPrompt(next);
          setError(current => current === CHECK_FAILED ? null : current);
        }
      } catch {
        if (!disposed) {
          setPrompt(null);
          setError(CHECK_FAILED);
        }
      }
      if (!disposed) timer = setTimeout(refresh, 1000);
    }
    void refresh();
    return () => { disposed = true; clearTimeout(timer); };
  }, []);

  async function answer(key: string, dismiss: boolean) {
    if (answering.current) return;
    answering.current = true;
    setBusy(true);
    setError(null);
    try {
      await invoke(dismiss ? 'dismiss_call_detection_prompt' : 'confirm_call_detection_prompt', { key });
      setPrompt(null);
    } catch {
      setError(dismiss ? 'Could not dismiss the prompt. Try again.' : 'Could not start recording. You can also answer from the tray.');
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
        <div className="flex gap-2">
          <Button disabled={busy} onClick={() => void answer(prompt.key, false)}>{prompt.retry ? 'Try again' : 'Record'}</Button>
          <Button variant="outline" disabled={busy} onClick={() => void answer(prompt.key, true)}>Not now</Button>
        </div>
      </>}
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
    </main>
  </>;
}
