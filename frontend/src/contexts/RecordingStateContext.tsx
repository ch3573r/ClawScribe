'use client';

import React, {createContext, useCallback, useContext, useEffect, useMemo, useState} from 'react';
import {recordingService} from '@/services/recordingService';

export enum RecordingStatus {
  IDLE = 'idle', STARTING = 'starting', RECORDING = 'recording', STOPPING = 'stopping',
  PROCESSING_TRANSCRIPTS = 'processing', SAVING = 'saving', COMPLETED = 'completed', ERROR = 'error',
}
interface RecordingLifecycle {
  isRecording: boolean;
  isPaused: boolean;
  isActive: boolean;
  status: RecordingStatus;
  statusMessage?: string;
}
interface RecordingClock { recordingDuration: number | null; activeDuration: number | null; }
interface RecordingContext extends RecordingLifecycle {
  setStatus: (status: RecordingStatus, message?: string) => void;
  isStarting: boolean; isStopping: boolean; isProcessing: boolean; isSaving: boolean;
}
const LifecycleContext = createContext<RecordingContext | null>(null);
const ClockContext = createContext<RecordingClock | null>(null);
export function useRecordingState(): RecordingContext {
  const state = useContext(LifecycleContext);
  if (!state) throw Error('useRecordingState requires RecordingStateProvider');
  return state;
}
/** Subscribe only the duration display to clock ticks, not every recording consumer. */
export function useRecordingClock(): RecordingClock {
  const clock = useContext(ClockContext);
  if (!clock) throw Error('useRecordingClock requires RecordingStateProvider');
  return clock;
}
const wholeSeconds = (value: number | null): number | null => value !== null && Number.isFinite(value) ? Math.max(0, Math.floor(value)) : null;

export function RecordingStateProvider({children}: {children: React.ReactNode}) {
  const [state, setState] = useState<RecordingLifecycle & RecordingClock>({
    isRecording:false, isPaused:false, isActive:false, status:RecordingStatus.IDLE,
    recordingDuration:null, activeDuration:null,
  });
  const setStatus = useCallback((status: RecordingStatus, message?: string) => {
    setState(previous => previous.status === status && previous.statusMessage === message ? previous : {...previous,status,statusMessage:message});
  }, []);

  useEffect(() => {
    let disposed = false;
    let generation = 0;
    let inFlight = false;
    let shouldPoll = false;
    let failed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const unsubscribers: (() => void)[] = [];
    const stopTimer = () => { if (timer !== undefined) clearTimeout(timer); timer=undefined; };
    const schedule = () => {
      if (!disposed && shouldPoll && timer === undefined) {
        timer = setTimeout(() => {timer=undefined; void sync();}, failed ? 2000 : 500);
      }
    };
    const sync = async () => {
      if (disposed || inFlight) return;
      inFlight = true;
      const epoch = generation;
      try {
        const backend = await recordingService.getRecordingState();
        if (disposed || epoch !== generation) return;
        shouldPoll = backend.is_recording;
        failed = false;
        const duration = wholeSeconds(backend.recording_duration);
        const active = wholeSeconds(backend.active_duration);
        setState(previous => {
          // Reloading during recording restores status AND polling. Do not
          // overwrite an explicitly active stop/save/error lifecycle.
          const status = backend.is_recording && [RecordingStatus.IDLE,RecordingStatus.COMPLETED].includes(previous.status)
            ? RecordingStatus.RECORDING : previous.status;
          if (previous.isRecording===backend.is_recording && previous.isPaused===backend.is_paused &&
              previous.isActive===backend.is_active && previous.recordingDuration===duration &&
              previous.activeDuration===active && previous.status===status) return previous;
          return {...previous,isRecording:backend.is_recording,isPaused:backend.is_paused,isActive:backend.is_active,
            recordingDuration:duration,activeDuration:active,status};
        });
      } catch {
        if (!disposed && epoch === generation) {
          if (!failed) console.warn('Recording state synchronization unavailable; retrying without discarding current state');
          failed = true;
          shouldPoll = true;
        }
      } finally {
        inFlight = false;
        schedule();
      }
    };
    const change = (update: (previous: RecordingLifecycle & RecordingClock) => RecordingLifecycle & RecordingClock, poll: boolean) => {
      if (disposed) return;
      generation++; // A delayed poll cannot overwrite a newer native event.
      shouldPoll = poll;
      stopTimer();
      setState(update);
      schedule();
    };
    const register = async (subscribe: () => Promise<() => void>) => {
      if (disposed) return;
      const unsubscribe = await subscribe();
      if (disposed) unsubscribe(); else unsubscribers.push(unsubscribe);
    };
    const setup = async () => {
      try {
        await register(() => recordingService.onRecordingStarted(() => change(previous => ({...previous,
          isRecording:true,isPaused:false,isActive:true,status:RecordingStatus.RECORDING,statusMessage:undefined,
          recordingDuration:0,activeDuration:0}),true)));
        await register(() => recordingService.onRecordingStopped(() => change(previous => {
          const status = [RecordingStatus.STOPPING,RecordingStatus.PROCESSING_TRANSCRIPTS,RecordingStatus.SAVING,RecordingStatus.ERROR].includes(previous.status)
            ? previous.status : RecordingStatus.STOPPING;
          return {...previous,isRecording:false,isPaused:false,isActive:false,recordingDuration:null,activeDuration:null,status,
            statusMessage:status===RecordingStatus.STOPPING?'Stopping recording…':previous.statusMessage};
        },false)));
        await register(() => recordingService.onRecordingPaused(() => change(previous => ({...previous,isPaused:true,isActive:false}),true)));
        await register(() => recordingService.onRecordingResumed(() => change(previous => ({...previous,isPaused:false,isActive:true}),true)));
      } catch {
        if (!disposed) console.warn('Some recording event listeners are unavailable; polling remains available');
      }
    };
    void setup();
    void sync();
    return () => {disposed=true; generation++; stopTimer(); unsubscribers.forEach(unsubscribe=>unsubscribe());};
  }, []);

  const lifecycle = useMemo(() => ({
    isRecording:state.isRecording,isPaused:state.isPaused,isActive:state.isActive,status:state.status,statusMessage:state.statusMessage,setStatus,
    isStarting:state.status===RecordingStatus.STARTING,isStopping:state.status===RecordingStatus.STOPPING,
    isProcessing:state.status===RecordingStatus.PROCESSING_TRANSCRIPTS,isSaving:state.status===RecordingStatus.SAVING,
  }), [state.isRecording,state.isPaused,state.isActive,state.status,state.statusMessage,setStatus]);
  const clock = useMemo(() => ({recordingDuration:state.recordingDuration,activeDuration:state.activeDuration}), [state.recordingDuration,state.activeDuration]);
  return <LifecycleContext.Provider value={lifecycle}><ClockContext.Provider value={clock}>{children}</ClockContext.Provider></LifecycleContext.Provider>;
}
