import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { errorText } from '@/lib/errorText';

export interface AudioFileInfo {
  path: string;
  filename: string;
  duration_seconds: number;
  size_bytes: number;
  format: string;
  modified_at?: string | null;
}

export interface ImportQueueItem {
  id: string;
  filename: string;
  title: string;
  status: 'queued' | 'running' | 'cancelling' | 'needs_source' | 'cancelled' | 'failed' | 'saved';
  message: string | null;
  result: {
    meeting_id: string;
    segments_count: number;
    duration_seconds: number;
    files_pending: boolean;
    transcription_provider?: string | null;
    transcription_incomplete?: boolean;
  } | null;
  elapsed_seconds: number | null;
  provider: string | null;
  requires_file: boolean;
  progress: { stage: string; message: string; progress_percentage: number } | null;
}

export function useImportQueue(open: boolean) {
  const [items, setItems] = useState<ImportQueueItem[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (!open) return;
    let active = true;
    let fetching = false;
    let refreshRequested = false;
    const refresh = async () => {
      if (!active) return;
      if (fetching) {
        refreshRequested = true;
        return;
      }
      fetching = true;
      try {
        const snapshot = await invoke<ImportQueueItem[]>('import_queue_snapshot');
        if (active) {
          setItems(snapshot);
          setError(null);
        }
      } catch (caught) {
        if (active) setError(errorText(caught, 'Could not read the import queue'));
      } finally {
        if (active) setLoading(false);
        fetching = false;
        if (refreshRequested) {
          refreshRequested = false;
          void refresh();
        }
      }
    };
    const subscriptions = ['import-queue-changed', 'transcript-files-synchronized'].map(event => listen(event, refresh));
    void Promise.allSettled(subscriptions).then(refresh);
    void refresh();
    const timer = window.setInterval(refresh, 1000);
    return () => {
      active = false;
      window.clearInterval(timer);
      for (const subscription of subscriptions) void subscription.then(unlisten => unlisten(), () => {});
    };
  }, [open]);

  return { items, error, loading };
}
