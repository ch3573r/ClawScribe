'use client';

import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { errorText } from '@/lib/errorText';

interface Run {
  run_id: number;
  revision: number;
  meeting_id: string;
  started_at_ms: number;
  replaces_transcript: boolean;
}

interface Progress {
  stage: string;
  progress_percentage: number;
  message: string;
}

interface Result {
  segments_count: number;
  duration_seconds: number;
  language: string | null;
  provider: string;
  model: string;
  fallback_reason?: string;
  file_warning: boolean;
}

type RetranscriptionStatus = Run & (
  | { state: 'running'; progress: Progress }
  | { state: 'complete'; result: Result; processing_seconds: number }
  | { state: 'failed'; error: string }
  | { state: 'cancelled' }
);

interface StartRequest {
  meetingId: string;
  meetingFolderPath: string;
  replacesTranscript: boolean;
  language: string | null;
  model: string;
  provider: string;
  cloudDestination: string | null;
}

interface RetranscriptionContextValue {
  status: RetranscriptionStatus | null;
  starting: Pick<StartRequest, 'meetingId' | 'replacesTranscript'> | null;
  cancellingRunId: number | null;
  ready: boolean;
  connectionError: string | null;
  reconnect: () => void;
  start: (request: StartRequest) => Promise<void>;
  cancel: (runId: number) => Promise<void>;
}

const RetranscriptionContext = createContext<RetranscriptionContextValue | null>(null);

export function useRetranscription() {
  const value = useContext(RetranscriptionContext);
  if (!value) throw new Error('RetranscriptionProvider is missing');
  return value;
}

export function RetranscriptionProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<RetranscriptionStatus | null>(null);
  const [starting, setStarting] = useState<RetranscriptionContextValue['starting']>(null);
  const [cancellingRunId, setCancellingRunId] = useState<number | null>(null);
  const [ready, setReady] = useState(false);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const latest = useRef<RetranscriptionStatus | null>(null);
  const startingRequest = useRef(false);
  const lastAnnouncement = useRef(0);

  useEffect(() => {
    let disposed = false;
    const subscriptions = ['retranscription-progress', 'retranscription-complete', 'retranscription-error']
      .map(event => listen<{ status: RetranscriptionStatus }>(event, ({ payload }) => {
        if (!disposed) acceptStatus(payload.status, true);
      }));

    // Subscribe first so a run finishing during the query still produces an event.
    void Promise.all(subscriptions).then(() => invoke<RetranscriptionStatus | null>('get_retranscription_status'))
      .then(snapshot => {
        if (disposed) return;
        if (snapshot) acceptStatus(snapshot);
        setReady(true);
      })
      .catch(() => {
        if (!disposed) setConnectionError('Could not load transcription status. Try again.');
      });

    return () => {
      disposed = true;
      subscriptions.forEach(subscription => { void subscription.then(unlisten => unlisten(), () => {}); });
    };
  }, [attempt]);

  function acceptStatus(snapshot: RetranscriptionStatus, announce = false) {
    if (!latest.current || snapshot.revision > latest.current.revision) {
      latest.current = snapshot;
      setStatus(snapshot);
      if (snapshot.state !== 'running') setCancellingRunId(id => id === snapshot.run_id ? null : id);
    }
    if (!announce || snapshot.state === 'running' || snapshot.revision <= lastAnnouncement.current) return;
    lastAnnouncement.current = snapshot.revision;
    const action = snapshot.replaces_transcript ? 'Retranscription' : 'Transcription';
    if (snapshot.state === 'complete') {
      const { segments_count } = snapshot.result;
      toast.success(`${action} complete! ${segments_count} segments created.`);
    } else if (snapshot.state === 'failed') {
      toast.error(`${action} failed`, { description: snapshot.error });
    } else {
      toast.info(`${action} cancelled`);
    }
  }

  async function start(request: StartRequest) {
    if (!ready || startingRequest.current || latest.current?.state === 'running') {
      throw new Error('Wait for the current transcription to finish.');
    }
    startingRequest.current = true;
    setStarting({ meetingId: request.meetingId, replacesTranscript: request.replacesTranscript });
    try {
      acceptStatus(await invoke<RetranscriptionStatus>('start_retranscription_command', { ...request }));
    } catch (caught) {
      const error = errorText(caught, 'Could not start transcription.');
      toast.error(request.replacesTranscript ? 'Retranscription failed' : 'Transcription failed', { description: error });
      throw caught;
    } finally {
      startingRequest.current = false;
      setStarting(null);
    }
  }

  async function cancel(runId: number) {
    if (latest.current?.state !== 'running' || latest.current.run_id !== runId || cancellingRunId === runId) return;
    setCancellingRunId(runId);
    try {
      if (await invoke<boolean>('cancel_retranscription_command', { runId })) return;
    } catch {
      if (latest.current?.run_id === runId && latest.current.state === 'running') {
        toast.error('Could not cancel transcription. Try again.');
      }
    }
    setCancellingRunId(id => id === runId ? null : id);
  }

  function reconnect() {
    setReady(false);
    setConnectionError(null);
    setAttempt(current => current + 1);
  }

  return <RetranscriptionContext.Provider value={{ status, starting, cancellingRunId, ready, connectionError, reconnect, start, cancel }}>
    {children}
  </RetranscriptionContext.Provider>;
}
