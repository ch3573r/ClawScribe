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
import { diskSpaceLowText, type DiskSpaceLowPayload } from '@/lib/disk-space';

const DISK_SPACE_TOAST = 'recording-disk-space-low';

/**
 * RecordingPostProcessingProvider
 *
 * App-wide owner of everything that ends a recording, whichever page is open:
 * - 'recording-stop-complete' from the tray, shortcut, overlay or detected call
 * - Stop from the sidebar indicator ('stop-recording-from-sidebar')
 * - 'recording-error' from the audio core
 *
 * Each of them runs the full post-processing flow (save to database, summary,
 * analytics) through the same stop handler.
 */
export function RecordingPostProcessingProvider({ children }: { children: React.ReactNode }) {
  // No-op functions since the global RecordingStateContext already handles state updates
  // These are only needed for the hook's local component state management
  const setIsRecording = () => { };
  const setIsRecordingDisabled = () => { };

  const {
    handleRecordingStop,
    setIsStopping,
  } = useRecordingStop(setIsRecording, setIsRecordingDisabled);
  const { isRecording, isStopping, status, setStatus } = useRecordingState();

  // The listeners below subscribe once. They read the latest handlers and state
  // from this ref instead of re-subscribing on every render.
  const latest = useRef({ handleRecordingStop, setIsStopping, isRecording, isStopping, status, setStatus });
  latest.current = { handleRecordingStop, setIsStopping, isRecording, isStopping, status, setStatus };
  const stopFlow = useRef<Promise<void> | null>(null);

  // Stop the recording and save it, exactly as the Stop button does.
  const stopRecording = useRef(async () => {
    if (stopFlow.current) return stopFlow.current;
    stopFlow.current = (async () => {
      const current = latest.current;
      if (!current.isRecording || current.isStopping) return;
      current.setIsStopping(true);
      try {
        const dataDir = await appDataDir();
        const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
        await recordingService.stopRecording(`${dataDir}/recording-${timestamp}.wav`);
        await latest.current.handleRecordingStop(true);
      } catch (error) {
        console.error('Failed to stop recording:', error);
        await latest.current.handleRecordingStop(false);
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

    // event.payload is the callApi boolean (true for normal stops). The stop
    // handler itself tells the core when the save it owns has ended.
    subscribe(listen<boolean>('recording-stop-complete', (event) => {
      void (async () => {
        try {
          await latest.current.handleRecordingStop(event.payload);
        } catch (error) {
          console.error('[RecordingPostProcessing] Post-stop save failed:', error);
        }
      })();
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
