import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { SummaryJobSnapshot } from './summary-job-status';

type Listener = (snapshot: SummaryJobSnapshot) => void | Promise<void>;
export type SubscribeToSummary = (listener: Listener, options?: { refresh?: boolean }) => () => void;

const RETRY_DELAYS_MS = [2000, 10000];

// One read and one set of native listeners for the editor and the generation
// flow. A new event during a read invalidates that read and queues the latest.
export function createSummaryUpdates(meetingId: string) {
  function connect() {
    const listeners = new Set<Listener>();
    let snapshot: SummaryJobSnapshot | undefined;
    let ready = false;
    let active = true;
    let version = 0;
    let reading = false;
    let retries = 0;
    let retryTimer: ReturnType<typeof setTimeout> | undefined;

    const cancelRetry = () => {
      clearTimeout(retryTimer);
      retryTimer = undefined;
    };
    const reportError = () => console.error('Could not apply a summary update');
    const notify = (listener: Listener, result: SummaryJobSnapshot) => {
      try { void Promise.resolve(listener(result)).catch(reportError); }
      catch { reportError(); }
    };
    const read = async (): Promise<void> => {
      reading = true;
      try {
        while (active) {
          const request = version;
          let result: SummaryJobSnapshot;
          try {
            result = await invoke<SummaryJobSnapshot>('api_get_summary', { meetingId });
          } catch {
            result = { status: 'unavailable', error: 'Could not read the saved summary.' };
          }
          if (!active) return;
          if (request !== version) continue;
          snapshot = result;
          for (const listener of listeners) notify(listener, result);
          if (request !== version) continue;
          // Completion can be the last event. Retry a failed read even when the
          // native job has already finished and will not emit anything else.
          if (active && result.status === 'unavailable') {
            const delay = RETRY_DELAYS_MS[retries++];
            if (delay !== undefined) retryTimer = setTimeout(() => {
              retryTimer = undefined;
              if (active) void read();
            }, delay);
          } else {
            retries = 0;
          }
          return;
        }
      } finally { reading = false; }
    };
    const refresh = () => {
      if (!active) return;
      cancelRetry();
      retries = 0;
      ++version;
      if (ready && !reading) void read();
    };
    const subscriptions = [
      ...['summary-job-changed', 'summary-job-progress'].map(event =>
        listen<string>(event, ({ payload }) => { if (payload === meetingId) refresh(); })),
      ...['transcript-text-edited', 'retranscription-complete', 'speaker-diarization-complete', 'transcript-speaker-updated'].map(event =>
        listen<{ meeting_id: string }>(event, ({ payload }) => { if (payload.meeting_id === meetingId) refresh(); })),
    ];
    void Promise.allSettled(subscriptions).then(() => {
      if (active) {
        ready = true;
        refresh();
      }
    });
    return {
      listeners,
      refresh,
      attach(listener: Listener, forceRefresh: boolean) {
        listeners.add(listener);
        if (forceRefresh) refresh();
        else if (snapshot && !reading) {
          const current = snapshot;
          const request = version;
          void Promise.resolve().then(() => {
            if (active && request === version && listeners.has(listener)) notify(listener, current);
          });
        }
      },
      disconnect() {
        active = false;
        cancelRetry();
        subscriptions.forEach(sub => void sub.then(off => off()).catch(() => {}));
      },
    };
  }

  let connection: ReturnType<typeof connect> | undefined;
  const subscribe: SubscribeToSummary = (listener, options) => {
    const current = connection ??= connect();
    current.attach(listener, options?.refresh === true);
    return () => {
      current.listeners.delete(listener);
      if (connection === current && current.listeners.size === 0) {
        current.disconnect();
        connection = undefined;
      }
    };
  };
  return { subscribe, refresh: () => connection?.refresh() };
}
