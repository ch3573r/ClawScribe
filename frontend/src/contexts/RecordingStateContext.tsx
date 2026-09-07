'use client';

import React, { createContext, useContext, useState, useEffect, useRef, useCallback, useMemo } from 'react';
import { recordingService } from '@/services/recordingService';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
export type RecordingMode = 'live' | 'audio_only';

/**
 * Recording state synchronized with backend
 * This context provides a single source of truth for recording state
 * that automatically syncs with the Rust backend, solving:
 * 1. Page refresh desync (backend recording but UI shows stopped)
 * 2. Pause state visibility across components
 * 3. Comprehensive state for future features (reconnection, etc.)
 */

// Recording lifecycle status enum
export enum RecordingStatus {
  IDLE = 'idle',                          // Not recording
  STARTING = 'starting',                  // Initiating recording
  RECORDING = 'recording',                // Active recording
  STOPPING = 'stopping',                  // Stop initiated, waiting for backend
  PROCESSING_TRANSCRIPTS = 'processing',  // Transcription completion wait
  SAVING = 'saving',                      // Saving to database
  COMPLETED = 'completed',                // Successfully saved
  ERROR = 'error'                         // Error occurred
}

interface RecordingState {
  sessionMode: RecordingMode;
  isRecording: boolean;           // Is a recording session active
  isPaused: boolean;              // Is the recording paused
  isActive: boolean;              // Is actively recording (recording && !paused)
  recordingDuration: number | null;  // Total duration including pauses
  activeDuration: number | null;     // Active recording time (excluding pauses)

  // NEW: Lifecycle status
  status: RecordingStatus;
  statusMessage?: string;  // Optional message for current status
}

interface RecordingStateContextType extends RecordingState {
  recordingMode: RecordingMode | null;
  modeError: string | null;
  isSavingMode: boolean;
  setRecordingMode: (mode: RecordingMode) => Promise<void>;
  // NEW: Setters for status management
  setStatus: (status: RecordingStatus, message?: string) => void;

  // Computed helpers (derived from status)
  isStarting: boolean;
  isStopping: boolean;
  isProcessing: boolean;
  isSaving: boolean;
}

const RecordingStateContext = createContext<RecordingStateContextType | null>(null);

export const useRecordingState = () => {
  const context = useContext(RecordingStateContext);
  if (!context) {
    throw new Error('useRecordingState must be used within a RecordingStateProvider');
  }
  return context;
};

export function RecordingStateProvider({ children }: { children: React.ReactNode }) {
  const [recordingMode, updateRecordingMode] = useState<RecordingMode | null>(null);
  const [modeError, setModeError] = useState<string | null>(null);
  const [isSavingMode, setIsSavingMode] = useState(false);
  const modeSave = useRef(false);
  const modeVersion = useRef(0);
  useEffect(() => {
    let active = true;
    const request = modeVersion.current;
    const subscription = listen<RecordingMode>('recording-mode-changed', event => {
      modeVersion.current++;
      if (active) { updateRecordingMode(event.payload); setModeError(null); }
    });
    void invoke<RecordingMode>('get_recording_mode').then(mode => {
      if (active && request === modeVersion.current) updateRecordingMode(mode);
    }).catch(() => { if (active && request === modeVersion.current) setModeError('Could not load recording mode. Choose a mode to retry.'); });
    void subscription.catch(() => { if (active) setModeError('Could not watch recording mode changes. Reopen ClawScribe to retry.'); });
    return () => { active = false; void subscription.then(unlisten => unlisten()).catch(() => {}); };
  }, []);
  const setRecordingMode = useCallback(async (mode: RecordingMode) => {
    if (modeSave.current) return;
    modeSave.current = true; setIsSavingMode(true); setModeError(null);
    try { await invoke('set_recording_mode', { mode }); modeVersion.current++; updateRecordingMode(mode); }
    catch { setModeError('Could not save recording mode. Please retry.'); }
    finally { modeSave.current = false; setIsSavingMode(false); }
  }, []);
  const [state, setState] = useState<RecordingState>({
    sessionMode: 'live',
    isRecording: false,
    isPaused: false,
    isActive: false,
    recordingDuration: null,
    activeDuration: null,
    status: RecordingStatus.IDLE,  // NEW: Initialize with IDLE status
    statusMessage: undefined,       // NEW: No message initially
  });

  const setStatus = useCallback((status: RecordingStatus, message?: string) => {
    setState(prev => ({ ...prev, status, statusMessage: message }));
  }, []);

  useEffect(() => {
    let active = true;
    let revision = 0;
    let syncing = false;
    let polling: ReturnType<typeof setInterval> | undefined;
    const unsubscribers: (() => void)[] = [];
    const stopPolling = () => {
      if (polling !== undefined) clearInterval(polling);
      polling = undefined;
    };
    const startPolling = () => {
      if (polling === undefined) polling = setInterval(() => { void sync(); }, 500);
    };
    const sync = async () => {
      if (!active || syncing) return;
      syncing = true;
      const request = revision;
      try {
        const backend = await recordingService.getRecordingState();
        // A stop/pause event is newer than a poll that began before it.
        if (!active || request !== revision) return;
        if (backend.is_recording) startPolling();
        else stopPolling();
        setState(prev => {
          const status = backend.is_recording && prev.status === RecordingStatus.IDLE
            ? RecordingStatus.RECORDING : prev.status;
          if (
            prev.status === status &&
            prev.sessionMode === (backend.recording_mode ?? 'live') &&
            prev.isRecording === backend.is_recording &&
            prev.isPaused === backend.is_paused &&
            prev.isActive === backend.is_active &&
            prev.recordingDuration === backend.recording_duration &&
            prev.activeDuration === backend.active_duration
          ) return prev;
          return {
            ...prev, status,
            sessionMode: backend.recording_mode ?? 'live',
            isRecording: backend.is_recording,
            isPaused: backend.is_paused,
            isActive: backend.is_active,
            recordingDuration: backend.recording_duration,
            activeDuration: backend.active_duration,
          };
        });
      } catch {
        if (active) console.error('Could not synchronize recording state.');
      } finally {
        syncing = false;
      }
    };
    const subscribe = async (subscription: Promise<() => void>) => {
      try {
        const unsubscribe = await subscription;
        if (active) unsubscribers.push(unsubscribe);
        else unsubscribe();
      } catch {
        if (active) console.error('Could not subscribe to recording state changes.');
      }
    };
    // Register all events before the initial snapshot, including after a WebView reload.
    void Promise.all([
      subscribe(recordingService.onRecordingStarted(mode => {
        if (!active) return;
        revision++;
        setState(prev => ({
          ...prev, sessionMode: mode, isRecording: true, isPaused: false,
          isActive: true, status: RecordingStatus.RECORDING, statusMessage: undefined,
        }));
        startPolling();
      })),
      subscribe(recordingService.onRecordingStopped(() => {
        if (!active) return;
        revision++;
        stopPolling();
        setState(prev => {
          const status = [RecordingStatus.STOPPING, RecordingStatus.PROCESSING_TRANSCRIPTS, RecordingStatus.SAVING]
            .includes(prev.status) ? prev.status : RecordingStatus.STOPPING;
          return {
            ...prev, status,
            statusMessage: status === RecordingStatus.STOPPING ? 'Stopping recording...' : prev.statusMessage,
            isRecording: false, isPaused: false, isActive: false,
            recordingDuration: null, activeDuration: null,
          };
        });
      })),
      subscribe(recordingService.onRecordingPaused(() => {
        if (!active) return;
        revision++;
        setState(prev => ({ ...prev, isPaused: true, isActive: false }));
      })),
      subscribe(recordingService.onRecordingResumed(() => {
        if (!active) return;
        revision++;
        setState(prev => ({ ...prev, isPaused: false, isActive: true }));
      })),
    ]).then(sync);
    return () => {
      active = false;
      stopPolling();
      unsubscribers.forEach(unsubscribe => unsubscribe());
    };
  }, []);

  // NEW: Computed helpers from status
  const contextValue = useMemo(() => ({
    recordingMode, modeError, isSavingMode, setRecordingMode,
    ...state,
    setStatus,
    isStarting: state.status === RecordingStatus.STARTING,
    isStopping: state.status === RecordingStatus.STOPPING,
    isProcessing: state.status === RecordingStatus.PROCESSING_TRANSCRIPTS,
    isSaving: state.status === RecordingStatus.SAVING,
  }), [state, setStatus, recordingMode, modeError, isSavingMode, setRecordingMode]);

  return (
    <RecordingStateContext.Provider value={contextValue}>
      {children}
    </RecordingStateContext.Provider>
  );
}
