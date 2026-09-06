"use client";

import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { AlertTriangle } from 'lucide-react';

/** Keep degraded capture visible after a toast expires, including across navigation. */
export function RecordingHealthBanner() {
  const [warning, setWarning] = useState<string | null>(null);
  useEffect(() => {
    let disposed = false;
    const subscriptions = [
      listen<string>('recording-warning', ({ payload }) => { if (!disposed) setWarning(payload); }),
      listen<string>('transcription-warning', () => {
        if (!disposed) setWarning('Live transcription is incomplete. After stopping, check the saved audio and use Retranscribe to recover missing speech.');
      }),
      listen('transcription-error', () => {
        if (!disposed) setWarning('Live transcription failed. Recording may still be running. Stop when ready, then check the saved audio and use Retranscribe.');
      }),
      listen('recording-started', () => { if (!disposed) setWarning(null); }),
      listen('recording-stopped', () => { if (!disposed) setWarning(null); }),
    ];
    return () => {
      disposed = true;
      subscriptions.forEach(subscription => { void subscription.then(unlisten => unlisten()).catch(() => {}); });
    };
  }, []);
  if (!warning) return null;
  return <div role="alert" className="flex shrink-0 items-start gap-2 border-b border-amber-500/40 bg-amber-500/10 px-4 py-3 text-sm text-foreground">
    <AlertTriangle aria-hidden="true" className="mt-0.5 size-4 shrink-0" />
    <p>{warning}</p>
  </div>;
}
