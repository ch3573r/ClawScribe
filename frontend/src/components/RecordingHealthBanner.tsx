"use client";

import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { AlertTriangle } from 'lucide-react';

/** Keep degraded capture visible after a toast expires, including across navigation. */
export function RecordingHealthBanner() {
  const [warnings, setWarnings] = useState<string[]>([]);
  useEffect(() => {
    let disposed = false;
    const add = (warning: string) => {
      if (!disposed && warning) setWarnings(current => current.includes(warning) ? current : [...current, warning]);
    };
    const subscriptions = [
      listen<string>('recording-warning', ({ payload }) => add(payload)),
      listen<string>('recording-warning-cleared', ({ payload }) => {
        if (!disposed) setWarnings(current => current.filter(warning => warning !== payload));
      }),
      listen<string>('transcription-warning', () => {
        add('Live transcription is incomplete. After stopping, check the saved audio and use Retranscribe to recover missing speech.');
      }),
      listen('transcription-error', () => {
        add('Live transcription failed. Recording may still be running. Stop when ready, then check the saved audio and use Retranscribe.');
      }),
      listen('recording-started', () => { if (!disposed) setWarnings([]); }),
      listen('recording-stopped', () => { if (!disposed) setWarnings([]); }),
    ];
    return () => {
      disposed = true;
      subscriptions.forEach(subscription => { void subscription.then(unlisten => unlisten()).catch(() => {}); });
    };
  }, []);
  if (!warnings.length) return null;
  return <div role="alert" className="flex shrink-0 items-start gap-2 border-b border-warning-border/40 bg-warning px-4 py-3 text-sm text-foreground">
    <AlertTriangle aria-hidden="true" className="mt-0.5 size-4 shrink-0" />
    <ul className="space-y-1">{warnings.map(warning => <li key={warning}>{warning}</li>)}</ul>
  </div>;
}
