'use client';

import React, { createContext, useContext, useState, useEffect, useRef, useCallback, useMemo, ReactNode, MutableRefObject } from 'react';
import { Transcript, TranscriptUpdate } from '@/types';
import { toast } from 'sonner';
import { useRecordingState } from './RecordingStateContext';
import { transcriptService } from '@/services/transcriptService';
import { recordingService } from '@/services/recordingService';
import { compareGeneration, recordingIdentity } from '@/lib/recording-identity';
import { indexedDBService } from '@/services/indexedDBService';
import { insertLiveTranscripts } from './live-transcript-order';

interface TranscriptContextType {
  transcripts: Transcript[];
  transcriptsRef: MutableRefObject<Transcript[]>
  addTranscript: (update: TranscriptUpdate) => void;
  copyTranscript: () => void;
  flushBuffer: () => void;
  meetingTitle: string;
  setMeetingTitle: (title: string) => void;
  clearTranscripts: () => void;
  currentMeetingId: string | null;
  markMeetingAsSaved: (id?: string | null) => Promise<void>;
}

const TranscriptContext = createContext<TranscriptContextType | undefined>(undefined);

type TranscriptEvent = TranscriptUpdate & { session_id?: string | null };

function toTranscript({ source, session_id, ...row }: TranscriptEvent): Transcript {
  const id = session_id ? `${session_id}-${row.sequence_id}` : String(row.sequence_id);
  return { ...row, id, speaker: source || undefined };
}

export function TranscriptProvider({ children }: { children: ReactNode }) {
  const [transcripts, setTranscripts] = useState<Transcript[]>([]);
  const [meetingTitle, setMeetingTitle] = useState('+ New Call');
  const [currentMeetingId, setCurrentMeetingId] = useState<string | null>(null);

  const { isRecording, sessionId } = useRecordingState();
  const transcriptsRef = useRef<Transcript[]>([]);
  const seenSequences = useRef(new Set<number>());
  const initializingCache = useRef<{ id: string; task: Promise<void> } | null>(null);
  const producer = useRef<{ sessionId: string; cacheId: string | null } | null>(null);
  const startedTitleSession = useRef<string | null>(null);
  const retiredSession = useRef<string | null>(null);
  const listenerReady = useRef<Promise<void> | null>(null);
  const finalFlushRef = useRef<(() => void) | null>(null);
  const adoptSession = useRef<((id: string, cacheId: string | null) => void) | null>(null);

  const replaceTranscripts = useCallback((rows: Transcript[]) => {
    transcriptsRef.current = rows;
    setTranscripts(rows);
  }, []);

  const appendTranscripts = useCallback((rows: Transcript[]) => {
    const unique = rows.filter(row => {
      if (row.sequence_id === undefined) return true;
      if (seenSequences.current.has(row.sequence_id)) return false;
      seenSequences.current.add(row.sequence_id);
      return true;
    });
    if (unique.length) replaceTranscripts(insertLiveTranscripts(transcriptsRef.current, unique));
  }, [replaceTranscripts]);

  const flushBuffer = useCallback(() => finalFlushRef.current?.(), []);

  const clearTranscripts = useCallback(() => {
    flushBuffer();
    retiredSession.current = producer.current?.sessionId ?? retiredSession.current;
    producer.current = null;
    seenSequences.current.clear();
    replaceTranscripts([]);
  }, [flushBuffer, replaceTranscripts]);

  // Keep both subscriptions for the provider's lifetime. The native worker can
  // deliver its first segment before Started reaches the webview.
  useEffect(() => {
    let active = true;
    let lastGeneration: string | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const pending = new Map<number, Transcript>();
    let early: TranscriptEvent[] = [];
    const unlisteners: (() => void)[] = [];

    const flush = () => {
      if (timer !== undefined) clearTimeout(timer);
      timer = undefined;
      appendTranscripts(Array.from(pending.values()));
      pending.clear();
    };
    finalFlushRef.current = flush;

    const receive = (owner: NonNullable<typeof producer.current>, update: TranscriptEvent) => {
      if (pending.has(update.sequence_id) || seenSequences.current.has(update.sequence_id)) return;
      pending.set(update.sequence_id, toTranscript(update));
      if (owner.cacheId) {
        void indexedDBService.saveTranscript(owner.cacheId, update)
          .catch(() => console.warn('Could not update the transcript recovery cache.'));
      }
      if (timer === undefined) timer = setTimeout(flush, 10);
    };

    adoptSession.current = (id, cacheId) => {
      if (producer.current?.sessionId !== id) {
        if (timer !== undefined) clearTimeout(timer);
        timer = undefined;
        pending.clear();
        seenSequences.current.clear();
        retiredSession.current = producer.current?.sessionId ?? retiredSession.current;
        replaceTranscripts([]);
      }
      const owner = { sessionId: id, cacheId };
      producer.current = owner;
      for (const update of early) {
        if (update.session_id === id) receive(owner, update);
      }
      early = [];
    };

    const subscribe = async (subscription: Promise<() => void>) => {
      const unlisten = await subscription;
      if (active) unlisteners.push(unlisten);
      else unlisten();
    };

    listenerReady.current = subscribe(transcriptService.onTranscriptUpdate((update: TranscriptEvent) => {
      if (!active) return;
      const owner = producer.current;
      if (owner && (update.session_id == null || update.session_id === owner.sessionId)) {
        receive(owner, update);
      } else if (update.session_id && update.session_id !== retiredSession.current) {
        // Native history also retains these rows. Bound the pre-admission
        // buffer while waiting for Started or the initial recording snapshot.
        if (early.length === 128) early.shift();
        early.push(update);
      }
    })).catch(() => {
      if (active) toast.error('Could not display live transcripts. Save your recording, then reopen ClawScribe to try again.');
    });

    void subscribe(recordingService.onRecordingStarted((_mode, id, generation, metadata) => {
      const identity = recordingIdentity(id, generation);
      const cacheId = metadata?.recording_id;
      if (!active || !identity || !metadata || !cacheId ||
          (lastGeneration && compareGeneration(identity.generation, lastGeneration) <= 0)) return;
      lastGeneration = identity.generation;
      setCurrentMeetingId(cacheId);
      const startedTitle = metadata.meeting_name?.trim();
      startedTitleSession.current = startedTitle ? identity.id : null;
      const title = startedTitle || 'New Meeting';
      setMeetingTitle(title);
      const task = indexedDBService.saveMeetingMetadata({
        meetingId: cacheId, title, folderPath: metadata.folder_path ?? undefined,
        startTime: Date.now(), lastUpdated: Date.now(), transcriptCount: 0, savedToSQLite: false,
      }).catch(() => console.warn('Could not initialize the transcript recovery cache.'));
      initializingCache.current = { id: cacheId, task };
      adoptSession.current?.(identity.id, cacheId);
      sessionStorage.setItem('indexeddb_current_meeting_id', cacheId);
    })).catch(() => console.warn('Could not subscribe to recording cache updates.'));

    return () => {
      active = false;
      if (timer !== undefined) clearTimeout(timer);
      finalFlushRef.current = null;
      adoptSession.current = null;
      listenerReady.current = null;
      unlisteners.forEach(unlisten => unlisten());
    };
  }, [appendTranscripts, replaceTranscripts]);

  // Populate history after the live listener is ready. Merge any gaps left by a
  // reload and prefer the live row if both paths saw the same sequence.
  useEffect(() => {
    if (!isRecording || !sessionId || retiredSession.current === sessionId) return;
    if (producer.current?.sessionId !== sessionId) {
      if (producer.current?.cacheId) return; // A newer native Started already arrived.
      adoptSession.current?.(sessionId, null);
    }
    let active = true;
    const isCurrent = () => active && producer.current?.sessionId === sessionId;
    const syncFromBackend = async () => {
      try {
        await listenerReady.current;
        if (!isCurrent()) return;
        const history = await transcriptService.getTranscriptHistory();
        if (!isCurrent()) return;
        flushBuffer();
        appendTranscripts(history.map(({ display_time, ...segment }) => ({
          ...segment,
          timestamp: display_time,
          chunk_start_time: segment.audio_start_time,
          is_partial: false,
          speaker: segment.speaker || undefined,
        })));

        if (startedTitleSession.current === sessionId) return;
        const meetingName = await recordingService.getRecordingMeetingName();
        if (isCurrent() && startedTitleSession.current !== sessionId && meetingName) setMeetingTitle(meetingName);
      } catch {
        if (isCurrent()) console.error('Could not synchronize transcript history.');
      }
    };

    void syncFromBackend();
    return () => { active = false; };
  }, [isRecording, sessionId, appendTranscripts, flushBuffer]);

  const addTranscript = useCallback((update: TranscriptUpdate) => {
    if (transcriptsRef.current.some(row => row.text === update.text && row.timestamp === update.timestamp)) return;
    appendTranscripts([toTranscript(update)]);
  }, [appendTranscripts]);

  // Copy transcript to clipboard with recording-relative timestamps
  const copyTranscript = useCallback(() => {
    // Format timestamps as recording-relative [MM:SS] instead of wall-clock time
    const formatTime = (seconds: number | undefined): string => {
      if (seconds === undefined) return '[--:--]';
      const totalSecs = Math.floor(seconds);
      const mins = Math.floor(totalSecs / 60);
      const secs = totalSecs % 60;
      return `[${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}]`;
    };

    const fullTranscript = transcripts
      .map(t => `${formatTime(t.audio_start_time)} ${t.text}`)
      .join('\n');
    navigator.clipboard.writeText(fullTranscript);

    toast.success("Transcript copied to clipboard");
  }, [transcripts]);

  const markMeetingAsSaved = useCallback(async (id?: string | null) => {
    if (!id) return;
    if (initializingCache.current?.id === id) await initializingCache.current.task;
    // The cache is disposable after the exact session commits in SQLite.
    await indexedDBService.markMeetingSaved(id);
    setCurrentMeetingId(current => current === id ? null : current);
    if (sessionStorage.getItem('indexeddb_current_meeting_id') === id) {
      sessionStorage.removeItem('indexeddb_current_meeting_id');
    }
  }, []);

  const value = useMemo<TranscriptContextType>(() => ({
    transcripts,
    transcriptsRef,
    addTranscript,
    copyTranscript,
    flushBuffer,
    meetingTitle,
    setMeetingTitle,
    clearTranscripts,
    currentMeetingId,
    markMeetingAsSaved,
  }), [transcripts, addTranscript, copyTranscript, flushBuffer, meetingTitle, clearTranscripts, currentMeetingId, markMeetingAsSaved]);

  return (
    <TranscriptContext.Provider value={value}>
      {children}
    </TranscriptContext.Provider>
  );
}

export function useTranscripts() {
  const context = useContext(TranscriptContext);
  if (context === undefined) {
    throw new Error('useTranscripts must be used within a TranscriptProvider');
  }
  return context;
}
