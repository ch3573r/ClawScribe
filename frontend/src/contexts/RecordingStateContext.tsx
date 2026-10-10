'use client';

import React, { createContext, useContext, useState, useEffect, useRef, useCallback, useMemo } from 'react';
import { recordingService } from '@/services/recordingService';
import { knowledgeService } from '@/services/knowledgeService';
import { recordingIdentity, compareGeneration, sameIdentity, type RecordingIdentity } from '@/lib/recording-identity';
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
  verificationError: string | null;
}

interface RecordingStateContextType extends RecordingState {
  recordingMode: RecordingMode | null;
  modeError: string | null;
  isSavingMode: boolean;
  setRecordingMode: (mode: RecordingMode) => Promise<void>;
  // NEW: Setters for status management
  setStatus: (status: RecordingStatus, message?: string) => void;
  subscribeLifecycle: (callback: (status: RecordingStatus) => void) => () => void;
  retryVerification: () => void;

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
    verificationError: null,
  });

  const lifecycleListeners = useRef(new Set<(status: RecordingStatus) => void>());
  const lifecycleStatus = useRef(RecordingStatus.IDLE);
  const lifecycleRevision = useRef(0);
  const retryStartWork = useRef<() => void>(() => {});
  const retryVerification = useCallback(() => retryStartWork.current(), []);
  const subscribeLifecycle = useCallback((callback: (status: RecordingStatus) => void) => {
    lifecycleListeners.current.add(callback);
    return () => { lifecycleListeners.current.delete(callback); };
  }, []);

  const setStatus = useCallback((status: RecordingStatus, message?: string) => {
    // Ephemeral assistance invalidates at the caller's first Stop, before any
    // React render or backend recording-stopped event can be awaited.
    lifecycleStatus.current = status;
    lifecycleRevision.current++;
    lifecycleListeners.current.forEach(callback => callback(status));
    setState(prev => ({ ...prev, status, statusMessage: message, verificationError: null }));
  }, []);

  useEffect(() => {
    let active = true;
    let revision = 0;
    let syncing = false;
    let backendSession: RecordingIdentity | null = null;
    let lifecycleOwner: RecordingIdentity | null = null;
    let stoppedGeneration: string | null = null;
    type StartCheck = { identity: RecordingIdentity; mode: RecordingMode; request: number; phase: number };
    let queuedStart: StartCheck | null = null;
    let retryStart: StartCheck | null = null;
    let checkingStart = false;
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
      const phase = lifecycleRevision.current;
      try {
        const backend = await recordingService.getRecordingState();
        // A stop/pause event is newer than a poll that began before it.
        if (!active || request !== revision || phase !== lifecycleRevision.current) return;
        const metadata = recordingIdentity(backend.session_id, backend.recording_generation);
        if (backend.is_recording && backend.session_id && !metadata) return;
        if (backend.is_recording && metadata && lifecycleOwner && !sameIdentity(lifecycleOwner, metadata)) return;
        if (backend.is_recording && backendSession === null && lifecycleStatus.current === RecordingStatus.IDLE) {
          // Bind reload recovery once too; rejected old start events must not
          // leave the stopped-event filter without the current identity.
          const snapshot = await knowledgeService.liveSnapshot().catch(() => null);
          if (!active || request !== revision || phase !== lifecycleRevision.current) return;
          const canonical = snapshot && recordingIdentity(snapshot.session_id, snapshot.recording_generation);
          if (!canonical || (metadata && !sameIdentity(metadata, canonical)) ||
            (stoppedGeneration && compareGeneration(canonical.generation, stoppedGeneration) <= 0)) return;
          backendSession = canonical;
          lifecycleOwner = canonical;
        }
        if (backend.is_recording) startPolling();
        else stopPolling();
        const status = backend.is_recording && lifecycleStatus.current === RecordingStatus.IDLE
          ? RecordingStatus.RECORDING : lifecycleStatus.current;
        lifecycleStatus.current = status;
        setState(prev => {
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
    const currentStart = (candidate: StartCheck) => active && candidate.request === revision &&
      candidate.phase === lifecycleRevision.current && sameIdentity(lifecycleOwner, candidate.identity);
    const failedStart = (candidate: StartCheck) => {
      if (!currentStart(candidate)) return;
      // Retain only the current failed candidate. It requires an explicit retry,
      // rather than entering the automatic newest-Started queue again.
      retryStart = candidate;
      setState(prev => ({ ...prev, verificationError: 'Could not verify the current recording. Retry status.' }));
    };
    const verifyStart = (): void => {
      if (!active || checkingStart || !queuedStart) return;
      const candidate = queuedStart;
      queuedStart = null;
      if (!currentStart(candidate)) return;
      checkingStart = true;
      void knowledgeService.liveSnapshot().then(snapshot => {
        if (!currentStart(candidate)) return;
        const canonical = recordingIdentity(snapshot.session_id, snapshot.recording_generation);
        if (!canonical || !sameIdentity(candidate.identity, canonical)) { failedStart(candidate); return; }
        retryStart = null;
        backendSession = canonical;
        revision++;
        lifecycleRevision.current++;
        lifecycleStatus.current = RecordingStatus.RECORDING;
        setState(prev => ({
          ...prev, sessionMode: candidate.mode, isRecording: true, isPaused: false,
          isActive: true, status: RecordingStatus.RECORDING, statusMessage: undefined, verificationError: null,
        }));
        startPolling();
      }).catch(() => failedStart(candidate)).finally(() => {
        checkingStart = false;
        verifyStart();
      });
    };
    const invalidateStart = () => { retryStart = null; queuedStart = null; };
    lifecycleListeners.current.add(invalidateStart);
    retryStartWork.current = () => {
      if (checkingStart || !retryStart || !currentStart(retryStart)) return;
      queuedStart = retryStart;
      retryStart = null;
      verifyStart();
    };
    // Register all events before the initial snapshot, including after a WebView reload.
    void Promise.all([
      subscribe(recordingService.onRecordingStarted((mode, sessionId, generation) => {
        if (!active) return;
        if ([RecordingStatus.STOPPING, RecordingStatus.PROCESSING_TRANSCRIPTS, RecordingStatus.SAVING,
          RecordingStatus.COMPLETED, RecordingStatus.ERROR].includes(lifecycleStatus.current)) return;
        const identity = recordingIdentity(sessionId, generation);
        if (!identity || (stoppedGeneration && compareGeneration(identity.generation, stoppedGeneration) <= 0)) return;
        if (lifecycleOwner) {
          const order = compareGeneration(identity.generation, lifecycleOwner.generation);
          if (order < 0 || (order === 0 && !sameIdentity(lifecycleOwner, identity))) return;
          if (order === 0 && (sameIdentity(backendSession, identity) || checkingStart || queuedStart)) return;
        }
        // Native generations order admitted producers independently of delivery
        // order. Only a canonical snapshot may promote the matching producer.
        lifecycleOwner = identity;
        revision++;
        retryStart = null;
        setState(prev => prev.verificationError === null ? prev : { ...prev, verificationError: null });
        queuedStart = { identity, mode, request: revision, phase: lifecycleRevision.current };
        verifyStart();
      })),
      subscribe(recordingService.onRecordingStopped(payload => {
        if (!active) return;
        const identity = recordingIdentity(payload?.session_id, payload?.recording_generation);
        if (payload?.recording_generation != null && !identity) return;
        if (identity) {
          if (stoppedGeneration && compareGeneration(identity.generation, stoppedGeneration) <= 0) return;
          if (lifecycleOwner) {
            const order = compareGeneration(identity.generation, lifecycleOwner.generation);
            if (order < 0 || (order === 0 && !sameIdentity(lifecycleOwner, identity))) return;
          }
          lifecycleOwner = identity;
          stoppedGeneration = identity.generation;
        } else {
          // Compatibility for older unidentified events; identified native
          // events always use the ordered producer pair above.
          if (payload?.session_id && lifecycleOwner && payload.session_id !== lifecycleOwner.id) return;
          stoppedGeneration = lifecycleOwner?.generation ?? stoppedGeneration;
        }
        queuedStart = null;
        retryStart = null;
        revision++;
        lifecycleRevision.current++;
        if (![RecordingStatus.STOPPING, RecordingStatus.PROCESSING_TRANSCRIPTS, RecordingStatus.SAVING]
          .includes(lifecycleStatus.current)) lifecycleStatus.current = RecordingStatus.STOPPING;
        stopPolling();
        lifecycleListeners.current.forEach(callback => callback(RecordingStatus.STOPPING));
        setState(prev => {
          const status = [RecordingStatus.STOPPING, RecordingStatus.PROCESSING_TRANSCRIPTS, RecordingStatus.SAVING]
            .includes(prev.status) ? prev.status : RecordingStatus.STOPPING;
          return {
            ...prev, status,
            statusMessage: status === RecordingStatus.STOPPING ? 'Stopping recording...' : prev.statusMessage,
            isRecording: false, isPaused: false, isActive: false,
            recordingDuration: null, activeDuration: null,
            verificationError: null,
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
      retryStartWork.current = () => {};
      lifecycleListeners.current.delete(invalidateStart);
      invalidateStart();
      stopPolling();
      unsubscribers.forEach(unsubscribe => unsubscribe());
    };
  }, []);

  // NEW: Computed helpers from status
  const contextValue = useMemo(() => ({
    recordingMode, modeError, isSavingMode, setRecordingMode,
    ...state,
    setStatus, subscribeLifecycle, retryVerification,
    isStarting: state.status === RecordingStatus.STARTING,
    isStopping: state.status === RecordingStatus.STOPPING,
    isProcessing: state.status === RecordingStatus.PROCESSING_TRANSCRIPTS,
    isSaving: state.status === RecordingStatus.SAVING,
  }), [state, setStatus, subscribeLifecycle, retryVerification, recordingMode, modeError, isSavingMode, setRecordingMode]);

  return (
    <RecordingStateContext.Provider value={contextValue}>
      {children}
    </RecordingStateContext.Provider>
  );
}
