'use client';

import React, { createContext, useContext, useState, useEffect, useRef, useCallback, ReactNode, MutableRefObject } from 'react';
import { Transcript, TranscriptUpdate } from '@/types';
import { toast } from 'sonner';
import { useRecordingState } from './RecordingStateContext';
import { transcriptService } from '@/services/transcriptService';
import { recordingService } from '@/services/recordingService';
import { compareGeneration, recordingIdentity } from '@/lib/recording-identity';
import { indexedDBService } from '@/services/indexedDBService';

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

export function TranscriptProvider({ children }: { children: ReactNode }) {
  const [transcripts, setTranscripts] = useState<Transcript[]>([]);
  const [meetingTitle, setMeetingTitle] = useState('+ New Call');
  const [currentMeetingId, setCurrentMeetingId] = useState<string | null>(null);

  // Recording state context - provides backend-synced state
  const recordingState = useRecordingState();

  // Refs for transcript management
  const transcriptsRef = useRef<Transcript[]>(transcripts);
  const initializingCache = useRef<{ id: string; task: Promise<void> } | null>(null);
  const finalFlushRef = useRef<(() => void) | null>(null);

  // Keep ref updated with current transcripts
  useEffect(() => {
    transcriptsRef.current = transcripts;
  }, [transcripts]);

  // The recovery cache uses the native session ID. A delayed cache cleanup
  // can address its own entry even after another recording starts.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let lastGeneration: string | undefined;

    void recordingService.onRecordingStarted((_mode, sessionId, generation, metadata) => {
      const identity = recordingIdentity(sessionId, generation);
      const id = metadata?.recording_id;
      if (disposed || !identity || !metadata || !id ||
          (lastGeneration && compareGeneration(identity.generation, lastGeneration) <= 0)) return;
      lastGeneration = identity.generation;
      setTranscripts([]);
      setCurrentMeetingId(id);
      sessionStorage.setItem('indexeddb_current_meeting_id', id);
      const title = metadata.meeting_name || 'New Meeting';
      setMeetingTitle(title);
      const task = indexedDBService.saveMeetingMetadata({
        meetingId: id, title, folderPath: metadata.folder_path ?? undefined,
        startTime: Date.now(), lastUpdated: Date.now(), transcriptCount: 0, savedToSQLite: false,
      }).catch(() => console.warn('Could not initialize the transcript recovery cache.'));
      initializingCache.current = { id, task };
    }).then(unsubscribe => {
      if (disposed) unsubscribe();
      else unlisten = unsubscribe;
    }).catch(() => console.warn('Could not subscribe to recording cache updates.'));

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // Main transcript buffering logic with sequence_id ordering
  useEffect(() => {
    let active = true;
    let unlistenFn: (() => void) | undefined;
    let transcriptCounter = 0;
    const transcriptBuffer = new Map<number, Transcript>();
    let lastProcessedSequence = 0;
    let processingTimer: NodeJS.Timeout | undefined;

    const processBufferedTranscripts = (forceFlush = false) => {
      const sortedTranscripts: Transcript[] = [];

      // Process all available sequential transcripts
      let nextSequence = lastProcessedSequence + 1;
      while (transcriptBuffer.has(nextSequence)) {
        const bufferedTranscript = transcriptBuffer.get(nextSequence)!;
        sortedTranscripts.push(bufferedTranscript);
        transcriptBuffer.delete(nextSequence);
        lastProcessedSequence = nextSequence;
        nextSequence++;
      }

      // Add any buffered transcripts that might be out of order
      const now = Date.now();
      const staleThreshold = 100;  // 100ms safety net only (serial workers = sequential order)
      const recentThreshold = 0;    // Show immediately - no delay needed with serial processing
      const staleTranscripts: Transcript[] = [];
      const recentTranscripts: Transcript[] = [];
      const forceFlushTranscripts: Transcript[] = [];

      for (const [sequenceId, transcript] of transcriptBuffer.entries()) {
        if (forceFlush) {
          // Force flush mode: process ALL remaining transcripts regardless of timing
          forceFlushTranscripts.push(transcript);
          transcriptBuffer.delete(sequenceId);
        } else {
          const transcriptAge = now - parseInt(transcript.id.split('-')[0]);
          if (transcriptAge > staleThreshold) {
            // Process stale transcripts (>100ms old - safety net)
            staleTranscripts.push(transcript);
            transcriptBuffer.delete(sequenceId);
          } else if (transcriptAge >= recentThreshold) {
            // Process immediately (0ms threshold with serial workers)
            recentTranscripts.push(transcript);
            transcriptBuffer.delete(sequenceId);
          }
        }
      }

      // Sort both stale and recent transcripts by chunk_start_time, then by sequence_id
      const sortTranscripts = (transcripts: Transcript[]) => {
        return transcripts.sort((a, b) => {
          const chunkTimeDiff = (a.chunk_start_time || 0) - (b.chunk_start_time || 0);
          if (chunkTimeDiff !== 0) return chunkTimeDiff;
          return (a.sequence_id || 0) - (b.sequence_id || 0);
        });
      };

      const sortedStaleTranscripts = sortTranscripts(staleTranscripts);
      const sortedRecentTranscripts = sortTranscripts(recentTranscripts);
      const sortedForceFlushTranscripts = sortTranscripts(forceFlushTranscripts);

      const allNewTranscripts = [...sortedTranscripts, ...sortedRecentTranscripts, ...sortedStaleTranscripts, ...sortedForceFlushTranscripts];

      if (allNewTranscripts.length > 0) {
        setTranscripts(prev => {
          // Create a set of existing sequence_ids for deduplication
          const existingSequenceIds = new Set(prev.map(t => t.sequence_id).filter(id => id !== undefined));

          // Filter out any new transcripts that already exist
          const uniqueNewTranscripts = allNewTranscripts.filter(transcript =>
            transcript.sequence_id !== undefined && !existingSequenceIds.has(transcript.sequence_id)
          );

          // Only combine if we have unique new transcripts
          if (uniqueNewTranscripts.length === 0) {
            return prev; // No new unique transcripts to add
          }


          // Merge with existing transcripts, maintaining chronological order
          const combined = [...prev, ...uniqueNewTranscripts];

          // Sort by chunk_start_time first, then by sequence_id
          return combined.sort((a, b) => {
            const chunkTimeDiff = (a.chunk_start_time || 0) - (b.chunk_start_time || 0);
            if (chunkTimeDiff !== 0) return chunkTimeDiff;
            return (a.sequence_id || 0) - (b.sequence_id || 0);
          });
        });
      }
    };

    // Assign final flush function to ref for external access
    finalFlushRef.current = () => processBufferedTranscripts(true);

    const setupListener = async () => {
      try {
        const unsubscribe = await transcriptService.onTranscriptUpdate((update) => {
          // Check for duplicate sequence_id before processing
          if (!active || transcriptBuffer.has(update.sequence_id)) {
            return;
          }

          // Create transcript for buffer with NEW timestamp fields
          const newTranscript: Transcript = {
            id: `${Date.now()}-${transcriptCounter++}`,
            text: update.text,
            speaker: update.source,
            timestamp: update.timestamp,
            sequence_id: update.sequence_id,
            chunk_start_time: update.chunk_start_time,
            is_partial: update.is_partial,
            confidence: update.confidence,
            // NEW: Recording-relative timestamps for playback sync
            audio_start_time: update.audio_start_time,
            audio_end_time: update.audio_end_time,
            duration: update.duration,
            word_timestamps: update.word_timestamps,
          };

          // Add to buffer
          transcriptBuffer.set(update.sequence_id, newTranscript);

          // Save to IndexedDB (non-blocking)
          if (currentMeetingId) {
            indexedDBService.saveTranscript(currentMeetingId, update)
              .catch(() => console.warn("IndexedDB save failed:"));
          }

          // Clear any existing timer and set a new one
          if (processingTimer) {
            clearTimeout(processingTimer);
          }

          // Process buffer with minimal delay for immediate UI updates (serial workers = sequential order)
          processingTimer = setTimeout(processBufferedTranscripts, 10);
        });
        if (active) unlistenFn = unsubscribe;
        else unsubscribe();
      } catch {
        if (!active) return;
        console.error("❌ Failed to setup MAIN transcript listener:");
        toast.error('Could not display live transcripts. Save your recording, then reopen ClawScribe to try again.');
      }
    };

    setupListener();

    return () => {
      active = false;
      if (processingTimer) {
        clearTimeout(processingTimer);
      }
      if (unlistenFn) {
        unlistenFn();
      }
    };
  }, [currentMeetingId]); // Add currentMeetingId dependency

  // Sync transcript history and meeting name from backend on reload
  // This fixes the issue where reloading during active recording causes state desync
  useEffect(() => {
    if (!recordingState.isRecording || transcriptsRef.current.length > 0) return;
    let active = true;
    const syncFromBackend = async () => {
      try {
        const history = await transcriptService.getTranscriptHistory();
        if (!active) return;
        const restored: Transcript[] = history.map(({ display_time, ...segment }) => ({
          ...segment,
          timestamp: display_time,
          chunk_start_time: segment.audio_start_time,
          is_partial: false,
          speaker: segment.speaker || undefined,
        }));

        // Live segments may have arrived while the snapshot was loading.
        setTranscripts(current => {
          if (!current.length) return restored;
          const received = new Set(current.map(row => row.sequence_id));
          const missing = restored.filter(row => !received.has(row.sequence_id));
          return missing.length ? [...missing, ...current].sort((a, b) =>
            (a.sequence_id ?? 0) - (b.sequence_id ?? 0)) : current;
        });

        const meetingName = await recordingService.getRecordingMeetingName();
        if (active && meetingName) setMeetingTitle(meetingName);
      } catch {
        if (active) console.error("[Reload Sync] Failed to sync from backend:");
      }
    };

    void syncFromBackend();
    return () => { active = false; };
  }, [recordingState.isRecording, recordingState.sessionId]);

  // Manual transcript update handler (for RecordingControls component)
  const addTranscript = useCallback((update: TranscriptUpdate) => {

    const newTranscript: Transcript = {
      id: update.sequence_id ? update.sequence_id.toString() : Date.now().toString(),
      text: update.text,
      timestamp: update.timestamp,
      sequence_id: update.sequence_id || 0,
      chunk_start_time: update.chunk_start_time,
      is_partial: update.is_partial,
      confidence: update.confidence,
      audio_start_time: update.audio_start_time,
      audio_end_time: update.audio_end_time,
      duration: update.duration,
      // Carry the source attribution (Me / Participants) so it persists to the
      // DB on save and renders as a badge in the finished transcript too — not
      // just live. Empty when source attribution is off.
      speaker: update.source || undefined,
    };

    setTranscripts(prev => {

      // Check if this transcript already exists
      const exists = prev.some(
        t => t.text === update.text && t.timestamp === update.timestamp
      );
      if (exists) {
        return prev;
      }

      // Add new transcript and sort by sequence_id to maintain order
      const updated = [...prev, newTranscript];
      const sorted = updated.sort((a, b) => (a.sequence_id || 0) - (b.sequence_id || 0));


      return sorted;
    });
  }, []);

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

  // Force flush buffer (for final transcript processing)
  const flushBuffer = useCallback(() => {
    if (finalFlushRef.current) {
      finalFlushRef.current();
    }
  }, []);

  // Clear transcripts (used when starting new recording)
  const clearTranscripts = useCallback(() => {
    setTranscripts([]);
    // Don't clear currentMeetingId here - it will be set by recording-started event
  }, []);

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

  const value: TranscriptContextType = {
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
  };

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
