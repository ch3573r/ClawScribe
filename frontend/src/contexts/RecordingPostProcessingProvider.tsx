'use client';

import React, { useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { appDataDir } from '@tauri-apps/api/path';
import { toast } from 'sonner';
import { useRecordingStop } from '@/hooks/useRecordingStop';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';
import { recordingService } from '@/services/recordingService';
import { captureHalted, recordingErrorMessage } from '@/lib/recording-capture';
import { STOP_FAILED_TITLE, stopFailureMessage } from '@/lib/stop-error';
import { diskSpaceLowText, type DiskSpaceLowPayload } from '@/lib/disk-space';

const DISK_SPACE_TOAST = 'recording-disk-space-low';

/** App-wide handling of native Stop completion and recording errors. */
export function RecordingPostProcessingProvider({ children }: { children: React.ReactNode }) {
  useRecordingStop();
  const { isRecording, isStopping, status, setStatus } = useRecordingState();
  const latest = useRef({ isRecording, isStopping, status, setStatus });
  latest.current = { isRecording, isStopping, status, setStatus };
  const stopFlow = useRef<Promise<void> | null>(null);

  // A Stop that failed leaves the recording running: say why and return to the
  // recording state at once. `onlyIfStopping` leaves other states alone.
  const showStopFailure = (error: unknown, onlyIfStopping = false) => {
    toast.error(STOP_FAILED_TITLE, { id: 'recording-stop-failed', description: stopFailureMessage(error) });
    if (!onlyIfStopping || latest.current.status === RecordingStatus.STOPPING) {
      latest.current.setStatus(RecordingStatus.RECORDING);
    }
  };

  // Stop the recording and save it, exactly as the Stop button does.
  const stopRecording = useRef(async () => {
    if (stopFlow.current) return stopFlow.current;
    stopFlow.current = (async () => {
      const current = latest.current;
      if (!current.isRecording || current.isStopping) return;
      current.setStatus(RecordingStatus.STOPPING);
      try {
        const dataDir = await appDataDir();
        const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
        await recordingService.stopRecording(`${dataDir}/recording-${timestamp}.wav`);
      } catch (error) {
        // The recording is still running: show why, skip the save flow and go
        // back to the recording state at once.
        console.error('Failed to stop recording:', error);
        showStopFailure(error);
      }
    })().finally(() => { stopFlow.current = null; });
    return stopFlow.current;
  }).current;

  useEffect(() => {
    let disposed = false;
    const unlisteners: (() => void)[] = [];
    const subscribe = (subscription: Promise<() => void>) => {
      subscription.then(unlisten => {
        if (disposed) unlisten();
        else unlisteners.push(unlisten);
      }).catch(error => {
        console.error('[RecordingPostProcessing] Failed to set up event listener:', error);
      });
    };

    // A Stop from the tray or the shortcuts failed in the core (for example the
    // audio device is not responding). The recording is still running.
    subscribe(listen<{ message?: string }>('recording-stop-failed', (event) => {
      if (!disposed) showStopFailure(event.payload?.message, true);
    }));

    // The capture core ends the session itself after a fatal error. Tell the
    // user, and finish the session so the interface does not keep showing a
    // recording that has stopped taking audio.
    subscribe(listen<string>('recording-error', async (event) => {
      const message = recordingErrorMessage(event.payload);
      let halted = false;
      let backendRecording = true;
      try {
        const backend = await recordingService.getRecordingState();
        backendRecording = backend.is_recording;
        halted = latest.current.status === RecordingStatus.RECORDING && captureHalted(backend);
      } catch {
        // Unknown state: report the error and leave the session alone.
      }
      if (disposed) return;
      if (!backendRecording && latest.current.status === RecordingStatus.STARTING) {
        latest.current.setStatus(RecordingStatus.IDLE);
      }
      toast.error(halted ? 'Recording stopped' : backendRecording ? 'Recording problem' : 'Recording failed', {
        id: 'recording-error',
        description: halted
          ? `${message}. ClawScribe is saving what was captured.`
          : backendRecording ? `${message}. Recording continues.` : `${message}.`,
        duration: Infinity,
      });
      if (halted) await stopRecording();
    }));

    // The recordings volume is running low. The recording continues, so this is a
    // warning that stays until it is dismissed or the recording ends.
    subscribe(listen<DiskSpaceLowPayload>('recording-disk-space-low', (event) => {
      toast.warning('Low disk space', {
        id: DISK_SPACE_TOAST,
        description: diskSpaceLowText(event.payload),
        duration: Infinity,
        action: {
          label: 'Open recordings folder',
          onClick: () => {
            invoke('open_recordings_folder').catch(() => toast.error('Could not open the folder.'));
          },
        },
      });
    }));
    // The warning is about the recording that has ended.
    subscribe(listen('recording-stopped', () => { toast.dismiss(DISK_SPACE_TOAST); }));

    // A recording takes priority over a running batch job. Say which job it stopped.
    subscribe(listen<{ job: string }>('batch-job-preempted', (event) => {
      const job = {
        retranscription: 'Retranscription',
        import: 'The import',
        'speaker-detection': 'Speaker detection',
        'pc-test': 'The PC test',
      }[event.payload?.job] ?? 'A running job';
      toast.info(`${job} is being stopped for the recording`, {
        description: 'Start it again after the recording if it did not finish.',
        duration: 10000,
      });
    }));

    // Stop from the sidebar indicator works from any page.
    const onStopFromSidebar = () => { void stopRecording(); };
    window.addEventListener('stop-recording-from-sidebar', onStopFromSidebar);

    return () => {
      disposed = true;
      window.removeEventListener('stop-recording-from-sidebar', onStopFromSidebar);
      unlisteners.forEach(unlisten => unlisten());
    };
  }, [stopRecording]);

  return <>{children}</>;
}
