'use client';

import React, { createContext, useContext, useState, useEffect, useRef, useCallback, ReactNode, MutableRefObject } from 'react';
import { Transcript, TranscriptUpdate } from '@/types';
import { toast } from 'sonner';
import { useRecordingState } from './RecordingStateContext';
import { transcriptService } from '@/services/transcriptService';
import { recordingService } from '@/services/recordingService';
import { OrderedTranscripts } from '@/lib/orderedTranscripts';
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
  markMeetingAsSaved: () => Promise<void>;
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
  const finalFlushRef = useRef<(() => void) | null>(null);
  const resetBufferRef = useRef<(() => void) | null>(null);
  const orderedRef = useRef(new OrderedTranscripts());
  const currentMeetingIdRef = useRef<string | null>(null);
  const recordingGenerationRef = useRef(0);
  const commitTranscripts = useCallback((items: readonly Transcript[], preserveExisting = false) => {
    const next = orderedRef.current.merge(items, preserveExisting);
    // Stop/save can read immediately, before React commits the next render.
    transcriptsRef.current = next;
    setTranscripts(next);
  }, []);

  // Initialize IndexedDB and listen for recording-started/stopped events
  useEffect(() => {
    let unlistenRecordingStarted: (() => void) | undefined;
    let unlistenRecordingStopped: (() => void) | undefined;
    let disposed = false;

    const setupRecordingListeners = async () => {
      try {
        // Initialize IndexedDB
        void indexedDBService.init().catch(() => console.warn('Transcript recovery storage is unavailable'));

        // Listen for recording-started event
        if (disposed) return;
        unlistenRecordingStarted = await recordingService.onRecordingStarted(async () => {
          if (disposed) return;
          try {
            // Generate unique meeting ID
            const meetingId = `meeting-${Date.now()}`;
            resetBufferRef.current?.();
            const empty = orderedRef.current.clear();
            transcriptsRef.current = empty;
            setTranscripts(empty);
            currentMeetingIdRef.current = meetingId;
            const generation = ++recordingGenerationRef.current;
            setCurrentMeetingId(meetingId);

            // Store in sessionStorage as fallback for markMeetingAsSaved
            sessionStorage.setItem('indexeddb_current_meeting_id', meetingId);
            console.log('[Recording Started] 💾 IndexedDB meeting ID stored:', meetingId);

            // Get meeting name
            const meetingName = await recordingService.getRecordingMeetingName();

            // Use a better fallback that matches the backend's naming pattern
            const effectiveTitle = meetingName || `Meeting ${new Date().toISOString().slice(0, 19).replace('T', '_').replace(/:/g, '-')}`;

            // Initialize meeting metadata in IndexedDB
            await indexedDBService.saveMeetingMetadata({
              meetingId,
              title: effectiveTitle,
              startTime: Date.now(),
              lastUpdated: Date.now(),
              transcriptCount: 0,
              savedToSQLite: false,
              folderPath: undefined // Will update shortly
            });

            // Synchronize meeting title to state (fixes tray stop title issue)
            if (!disposed && generation === recordingGenerationRef.current) setMeetingTitle(effectiveTitle);

            if (disposed || generation !== recordingGenerationRef.current) return;
            // Fetch folder path from backend and update metadata
            // This ensures folder path is persisted even if app crashes
            try {
              const { invoke } = await import('@tauri-apps/api/core');
              const folderPath = await invoke<string>('get_meeting_folder_path');
              if (folderPath && !disposed && generation === recordingGenerationRef.current) {
                const metadata = await indexedDBService.getMeetingMetadata(meetingId);
                if (metadata) {
                  metadata.folderPath = folderPath;
                  await indexedDBService.saveMeetingMetadata(metadata);
                }
              }
            } catch (error) {
              // Non-fatal - will be set on stop if recording completes normally
            }
          } catch (error) {
            console.error('Failed to initialize meeting in IndexedDB:', error);
          }
        });

        if (disposed) { unlistenRecordingStarted(); return; }
        // Listen for recording-stopped event
        unlistenRecordingStopped = await recordingService.onRecordingStopped(async (payload) => {
          if (disposed) return;
          try {
            const currentMeetingId = currentMeetingIdRef.current;
            if (currentMeetingId) {
              // Update folder path in IndexedDB
              const metadata = await indexedDBService.getMeetingMetadata(currentMeetingId);

              if (metadata && payload.folder_path) {
                metadata.folderPath = payload.folder_path;
                await indexedDBService.saveMeetingMetadata(metadata);
              }
            }
          } catch (error) {
            console.error('Failed to update meeting metadata on stop:', error);
          }
        });
        if (disposed) unlistenRecordingStopped();
      } catch (error) {
        console.error('Failed to setup recording listeners:', error);
      }
    };

    setupRecordingListeners();

    return () => {
      disposed = true;
      if (unlistenRecordingStarted) {
        unlistenRecordingStarted();
        console.log('🧹 Recording started listener cleaned up');
      }
      if (unlistenRecordingStopped) {
        unlistenRecordingStopped();
        console.log('🧹 Recording stopped listener cleaned up');
      }
    };
  }, []);

  // One listener for the provider lifetime. A bounded delay batches IPC bursts
  // without rebuilding listener state or starving updates during sustained input.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const pending = new Map<number, Transcript>();
    const flush = () => {
      if (timer !== undefined) clearTimeout(timer);
      timer = undefined;
      if (!pending.size || disposed) return;
      const batch = [...pending.values()];
      pending.clear();
      commitTranscripts(batch);
    };
    const reset = () => {
      if (timer !== undefined) clearTimeout(timer);
      timer = undefined;
      pending.clear();
    };
    finalFlushRef.current = flush;
    resetBufferRef.current = reset;
    void transcriptService.onTranscriptUpdate(update => {
      if (disposed) return;
      const previous = pending.get(update.sequence_id);
      if (previous && !previous.is_partial && update.is_partial) return;
      pending.set(update.sequence_id, {
        id: `transcript-${recordingGenerationRef.current}-${update.sequence_id}`,
        text: update.text, speaker: update.source, timestamp: update.timestamp,
        sequence_id: update.sequence_id, chunk_start_time: update.chunk_start_time,
        is_partial: update.is_partial, confidence: update.confidence,
        audio_start_time: update.audio_start_time, audio_end_time: update.audio_end_time,
        duration: update.duration, word_timestamps: update.word_timestamps,
      });
      const meetingId = currentMeetingIdRef.current;
      if (meetingId) void indexedDBService.saveTranscript(meetingId, update)
        .catch(() => console.warn('Transcript recovery persistence failed'));
      if (pending.size >= 128) flush();
      else if (timer === undefined) timer = setTimeout(flush, 50);
    }).then(removeListener => {
      if (disposed) removeListener();
      else unlisten = removeListener;
    }).catch(() => {
      if (!disposed) toast.error('Live transcript updates are unavailable. Your recording may still be active.');
    });
    return () => {
      flush();
      disposed = true;
      reset();
      unlisten?.();
      if (finalFlushRef.current === flush) finalFlushRef.current = null;
      if (resetBufferRef.current === reset) resetBufferRef.current = null;
    };
  }, [commitTranscripts]);

  // Sync transcript history and meeting name from backend on reload
  // This fixes the issue where reloading during active recording causes state desync
  useEffect(() => {
    let disposed = false;
    const generation = recordingGenerationRef.current;
    const syncFromBackend = async () => {
      // If recording is active and we have no local transcripts, sync from backend
      if (recordingState.isRecording && transcripts.length === 0) {
        try {
          console.log('[Reload Sync] Recording active after reload, syncing transcript history...');

          // Fetch transcript history from backend
          const history = await transcriptService.getTranscriptHistory();
          console.log(`[Reload Sync] Retrieved ${history.length} transcript segments from backend`);

          // Convert backend format to frontend Transcript format
          const formattedTranscripts: Transcript[] = history.map((segment: any) => ({
            id: segment.id,
            text: segment.text,
            timestamp: segment.display_time, // Use display_time for UI
            sequence_id: segment.sequence_id,
            chunk_start_time: segment.audio_start_time,
            is_partial: false, // History segments are always final
            confidence: segment.confidence,
            audio_start_time: segment.audio_start_time,
            audio_end_time: segment.audio_end_time,
            duration: segment.duration,
            speaker: segment.speaker || undefined,
          }));

          if (disposed || generation !== recordingGenerationRef.current) return;
          commitTranscripts(formattedTranscripts, true);
          console.log('[Reload Sync] ✅ Transcript history synced successfully');

          // Fetch meeting name from backend
          const meetingName = await recordingService.getRecordingMeetingName();
          if (meetingName && !disposed && generation === recordingGenerationRef.current) {
            console.log('[Reload Sync] Retrieved meeting name:', meetingName);
            setMeetingTitle(meetingName);
            console.log('[Reload Sync] ✅ Meeting title synced successfully');
          }
        } catch (error) {
          console.error('[Reload Sync] Failed to sync from backend:', error);
        }
      }
    };

    syncFromBackend();
    return () => { disposed = true; };
  }, [recordingState.isRecording, commitTranscripts]); // Run when recording state changes

  // Same index and final/partial semantics for manual updates and IPC events.
  const addTranscript = useCallback((update: TranscriptUpdate) => {
    commitTranscripts([{
      id: `transcript-${recordingGenerationRef.current}-${update.sequence_id}`,
      text: update.text, timestamp: update.timestamp, speaker: update.source,
      sequence_id: update.sequence_id, chunk_start_time: update.chunk_start_time,
      is_partial: update.is_partial, confidence: update.confidence,
      audio_start_time: update.audio_start_time, audio_end_time: update.audio_end_time,
      duration: update.duration, word_timestamps: update.word_timestamps,
    }]);
  }, [commitTranscripts]);

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
    void navigator.clipboard.writeText(fullTranscript)
      .then(() => toast.success("Transcript copied to clipboard"))
      .catch(() => toast.error("Could not copy the transcript. Please try again."));
  }, [transcripts]);

  // Force flush buffer (for final transcript processing)
  const flushBuffer = useCallback(() => {
    if (finalFlushRef.current) {
      console.log('🔄 Flushing transcript buffer...');
      finalFlushRef.current();
    }
  }, []);

  // Clear transcripts (used when starting new recording)
  const clearTranscripts = useCallback(() => {
    resetBufferRef.current?.();
    recordingGenerationRef.current++;
    const empty = orderedRef.current.clear();
    transcriptsRef.current = empty;
    setTranscripts(empty);
    // Don't clear currentMeetingId here - it will be set by recording-started event
  }, []);

  // Mark current meeting as saved in IndexedDB
  const markMeetingAsSaved = useCallback(async () => {
    // Try context state first, fallback to sessionStorage
    const meetingId = currentMeetingId || sessionStorage.getItem('indexeddb_current_meeting_id');

    if (!meetingId) {
      console.error('[IndexedDB] ❌ Cannot mark meeting as saved: No meeting ID available!');
      console.error('[IndexedDB] currentMeetingId:', currentMeetingId);
      console.error('[IndexedDB] sessionStorage:', sessionStorage.getItem('indexeddb_current_meeting_id'));
      return;
    }

    try {
      await indexedDBService.markMeetingSaved(meetingId);

      // Clear both sources
      setCurrentMeetingId(null);
      sessionStorage.removeItem('indexeddb_current_meeting_id');
    } catch (error) {
      console.error('[IndexedDB] ❌ Failed to mark meeting as saved:', error);
    }
  }, [currentMeetingId]);

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
