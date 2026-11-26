'use client';

import { invoke } from '@tauri-apps/api/core';
import { appDataDir } from '@tauri-apps/api/path';
import { useCallback, useEffect, useState } from 'react';
import { Play, Pause, Square, Mic, AlertCircle, X, PictureInPicture2 } from 'lucide-react';
import { listen } from '@tauri-apps/api/event';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import Analytics from '@/lib/analytics';
import { RecordingStatus, useRecordingElapsedSeconds, useRecordingState } from '@/contexts/RecordingStateContext';
import { LiveBookmarkButton } from '@/components/MeetingDetails/MeetingBookmarks';
import { LiveRecordingNotes } from '@/components/LiveRecordingNotes';
import { Button } from '@/components/ui/button';
import { RecordingLevelMeter } from '@/components/RecordingLevelMeter';
import { describeStartError } from '@/lib/start-error';
import { STOP_FAILED_TITLE, stopFailureMessage } from '@/lib/stop-error';
import { recordingService } from '@/services/recordingService';
import { toast } from 'sonner';
import { useActiveGlobalShortcuts } from '@/hooks/useActiveGlobalShortcuts';
import { shortcutHint } from '@/lib/shortcuts';

interface RecordingControlsProps {
  isRecording: boolean;
  onRecordingStart: () => void;
  onTranscriptionError?: (message: string) => void;
  onStopInitiated?: () => void; // Called immediately when stop button is clicked
  isRecordingDisabled: boolean;
  startDisabledReason?: string | null;
  isParentProcessing: boolean;
  selectedDevices?: {
    micDevice: string | null;
    systemDevice: string | null;
  };
  meetingName?: string;
  variant?: 'floating' | 'dashboard';
}

function RecordingElapsedTime() {
  const elapsed = useRecordingElapsedSeconds();
  const hours = Math.floor(elapsed / 3600);
  const minutes = Math.floor(elapsed / 60) % 60;
  const seconds = String(elapsed % 60).padStart(2, '0');
  const time = hours > 0 ? `${hours}:${String(minutes).padStart(2, '0')}:${seconds}` : `${minutes}:${seconds}`;
  return (
    <span className="font-mono text-sm tabular-nums text-foreground min-w-[3.25rem] text-center">
      {time}
    </span>
  );
}

export const RecordingControls: React.FC<RecordingControlsProps> = ({
  isRecording,
  onRecordingStart,
  onTranscriptionError,
  onStopInitiated,
  isRecordingDisabled,
  startDisabledReason,
  isParentProcessing,
  variant = 'floating',
}) => {
  // Use global recording state context for pause state (syncs with tray operations)
  const recordingState = useRecordingState();
  const isPaused = recordingState.isPaused;
  const shortcuts = useActiveGlobalShortcuts();

  const [isProcessing, setIsProcessing] = useState(false);
  const [isStarting, setIsStarting] = useState(false);
  const [isStopping, setIsStopping] = useState(false);
  const [isPausing, setIsPausing] = useState(false);
  const [isResuming, setIsResuming] = useState(false);
  const [openingControls, setOpeningControls] = useState(false);
  const [deviceError, setDeviceError] = useState<{
    title: string;
    message: string;
  } | null>(null);
  const isStartPending =
    isStarting ||
    recordingState.status === RecordingStatus.STARTING;

  useEffect(() => {
    const checkTauri = async () => {
      try {
        await invoke('is_recording');
      } catch {
        console.error("Tauri initialization error:");
        toast.error('Could not initialize recording. Reopen ClawScribe, then try again.');
      }
    };
    checkTauri();
  }, []);

  const handleStartRecording = useCallback(async () => {
    if (isStartPending || isRecordingDisabled) return;

    setIsStarting(true);

    try {
      // Start through the shared lifecycle hook. The backend validates the
      // selected transcription engine before opening devices and emits any
      // model/download errors as structured events.
      await onRecordingStart();
    } catch (error) {
      console.error("Failed to start recording:");
      console.error("Error details:");

      setDeviceError(describeStartError(error));
    } finally {
      setIsStarting(false);
    }
  }, [onRecordingStart, isStartPending, isRecordingDisabled]);

  const stopRecordingAction = useCallback(async () => {
    try {
      setIsProcessing(true);
      const dataDir = await appDataDir();
      const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
      const savePath = `${dataDir}/recording-${timestamp}.wav`;
      const stopped = await recordingService.stopRecording(savePath);
      if (stopped === false) {
        if (await recordingState.reconcileNothingToStop(stopped)) return;
        throw new Error('Could not verify whether the recording stopped. Try again.');
      }
      // Track successful transcription
      Analytics.trackTranscriptionSuccess();
    } catch (error) {
      if (await recordingState.reconcileNothingToStop(error)) return;
      console.error('Failed to stop recording');
      // The recording is still running: say why, skip the save flow, and go
      // back to the recording state at once instead of waiting for a poll.
      toast.error(STOP_FAILED_TITLE, { description: stopFailureMessage(error) });
      recordingState.setStatus(RecordingStatus.RECORDING);
    } finally {
      setIsProcessing(false);
      setIsStopping(false);
    }
  }, [recordingState]);

  const handleStopRecording = useCallback(async () => {
    if (!isRecording || isStarting || isStopping || isRecordingDisabled) {
      return;
    }


    // Notify parent immediately (for UI state updates)
    onStopInitiated?.();

    setIsStopping(true);

    // Immediately trigger the stop action
    await stopRecordingAction();
  }, [
    isRecording,
    isStarting,
    isStopping,
    isRecordingDisabled,
    stopRecordingAction,
    onStopInitiated,
  ]);

  const handlePauseRecording = useCallback(async () => {
    if (!isRecording || isPaused || isPausing || isRecordingDisabled) return;

    setIsPausing(true);

    try {
      await invoke('pause_recording');
      // isPaused state now managed by RecordingStateContext via events
    } catch {
      console.error("Failed to pause recording:");
      toast.error('Could not pause recording. Try pausing again or stop the recording.');
    } finally {
      setIsPausing(false);
    }
  }, [isRecording, isPaused, isPausing, isRecordingDisabled]);

  const handleResumeRecording = useCallback(async () => {
    if (!isRecording || !isPaused || isResuming || isRecordingDisabled) return;

    setIsResuming(true);

    try {
      await invoke('resume_recording');
      // isPaused state now managed by RecordingStateContext via events
    } catch {
      console.error("Failed to resume recording:");
      toast.error('Could not resume recording. Try resuming again or stop the recording.');
    } finally {
      setIsResuming(false);
    }
  }, [isRecording, isPaused, isResuming, isRecordingDisabled]);

  useEffect(() => {
    const unsubscribes: (() => void)[] = [];
    let disposed = false;

    const setupListeners = async () => {
      try {
        // Transcript error listener - handles both regular and actionable errors
        const transcriptErrorUnsubscribe = await listen(
          'transcript-error',
          (event) => {
            if (disposed) return;
            console.error("Transcription error received:");
            const errorMessage = event.payload as string;

            Analytics.trackTranscriptionError(errorMessage);

            setIsProcessing(false);
            if (onTranscriptionError) {
              onTranscriptionError(errorMessage);
            }
          },
        );
        if (disposed) return transcriptErrorUnsubscribe();
        unsubscribes.push(transcriptErrorUnsubscribe);

        // Transcription error listener - handles structured error objects with actionable flag
        const transcriptionErrorUnsubscribe = await listen(
          'transcription-error',
          (event) => {
            if (disposed) return;
            console.error("Transcription error received:");

            let errorMessage: string;

            if (typeof event.payload === 'object' && event.payload !== null) {
              const payload = event.payload as {
                error: string;
                userMessage: string;
              };
              errorMessage = payload.userMessage || payload.error;
            } else {
              errorMessage = String(event.payload);
            }

            Analytics.trackTranscriptionError(errorMessage);

            setIsProcessing(false);

            // For actionable errors (like model loading failures), the main page will handle showing the model selector
            // For regular errors, they are handled by useModalState global listener which shows a toast
            // Skip the callback so these errors don't show both a modal and a toast.
          },
        );

        if (disposed) return transcriptionErrorUnsubscribe();
        unsubscribes.push(transcriptionErrorUnsubscribe);
      } catch {
        console.error("Failed to set up recording event listeners:");
      }
    };

    setupListeners();

    return () => {
      disposed = true;
      unsubscribes.forEach(unsubscribe => unsubscribe());
    };
  }, [onTranscriptionError]);

  const isDashboard = variant === 'dashboard';
  const controlsClassName = isDashboard
    ? 'flex items-center gap-3 bg-transparent p-0 shadow-none'
    : 'flex items-center space-x-2 bg-card rounded-full shadow-sm px-4 py-2';
  const startButtonClassName = isDashboard
    ? `h-16 w-16 p-0 [&_svg]:!size-[26px] flex items-center justify-center rounded-full text-primary-foreground hover:text-primary-foreground shadow-[0_0_34px_hsl(var(--primary)/0.42)] transition-all relative ${isStartPending || isProcessing || isRecordingDisabled ? 'bg-muted cursor-not-allowed' : 'bg-gradient-to-br from-primary to-primary/70 hover:scale-105 hover:shadow-[0_0_44px_hsl(var(--primary)/0.6)]'}`
    : `w-12 h-12 p-0 [&_svg]:!size-5 flex items-center justify-center ${isStartPending || isProcessing ? 'bg-muted text-foreground hover:text-foreground' : 'bg-destructive hover:bg-destructive/90 text-destructive-foreground hover:text-destructive-foreground'} rounded-full transition-colors relative`;
  const secondaryButtonClassName = isDashboard
    ? `h-12 w-12 p-0 flex items-center justify-center rounded-full border border-border text-foreground hover:text-accent-foreground transition-colors relative ${isPausing || isResuming || isStopping ? 'bg-muted text-muted-foreground hover:text-muted-foreground' : 'bg-muted hover:bg-accent'}`
    : `w-10 h-10 p-0 flex items-center justify-center ${isPausing || isResuming || isStopping ? 'bg-muted border-2 border-border text-muted-foreground hover:text-muted-foreground' : 'bg-card border-2 border-border text-muted-foreground hover:text-muted-foreground hover:border-ring hover:bg-muted'} rounded-full transition-colors relative`;
  const stopButtonClassName = isDashboard
    ? `h-12 w-12 p-0 flex items-center justify-center rounded-full transition-colors relative ${isStopping || isPausing || isResuming ? 'bg-muted cursor-not-allowed text-foreground hover:text-foreground' : 'bg-destructive hover:bg-destructive/80 text-destructive-foreground hover:text-destructive-foreground'}`
    : `w-10 h-10 p-0 flex items-center justify-center ${isStopping || isPausing || isResuming ? 'bg-muted text-foreground hover:text-foreground' : 'bg-destructive hover:bg-destructive/90 text-destructive-foreground hover:text-destructive-foreground'} rounded-full transition-colors relative`;
  const waveformClassName = isDashboard
    ? 'flex items-center justify-center gap-1.5 rounded-full border border-primary/30 bg-primary/10 px-4 py-3'
    : 'flex items-center space-x-1 mx-4';

  return (
    <TooltipProvider>
      <div className="flex flex-col space-y-2">
        <div className={controlsClassName}>
          {isProcessing && !isParentProcessing ? (
            <div
              className={
                isDashboard
                  ? 'flex items-center gap-3 rounded-full border border-border bg-muted px-4 py-3 text-foreground'
                  : 'flex items-center space-x-2'
              }
            >
              <div
                className={
                  isDashboard
                    ? 'h-5 w-5 animate-spin rounded-full border-b-2 border-primary'
                    : 'animate-spin rounded-full h-5 w-5 border-b-2 border-foreground'
                }
              ></div>
              <span className="text-sm text-muted-foreground">
                Processing recording...
              </span>
            </div>
          ) : (
            <>
              {!isRecording ? (
                // Start recording button
                <Tooltip>
                  <TooltipTrigger asChild>
                    <span className="inline-flex rounded-full focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                      tabIndex={isRecordingDisabled || isStartPending ? 0 : undefined} aria-label={startDisabledReason ?? undefined}>
                      <Button variant="ghost" size="icon"
                        aria-label={isStartPending ? 'Starting recording' : 'Start recording'}
                        onClick={() => {
                          Analytics.trackButtonClick('start_recording', 'recording_controls');
                          void handleStartRecording();
                        }}
                        disabled={isProcessing || isRecordingDisabled || isStartPending}
                        className={startButtonClassName}
                      >
                        {isStartPending ? (
                          <div className="animate-spin rounded-full h-5 w-5 border-b-2 border-primary-foreground"></div>
                        ) : (
                          <Mic size={isDashboard ? 26 : 20} />
                        )}
                      </Button>
                    </span>
                  </TooltipTrigger>
                  <TooltipContent>
                    <p>{startDisabledReason ?? (isStartPending ? 'Starting recording…' : shortcutHint('Start recording', shortcuts.startStop))}</p>
                  </TooltipContent>
                </Tooltip>
              ) : (
                // Recording controls (pause/resume + stop)
                <>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button variant="ghost" size="icon" className={secondaryButtonClassName}
                        aria-label="Open floating controls" disabled={!recordingState.sessionId || openingControls || isRecordingDisabled || isStopping}
                        onClick={async () => {
                          setOpeningControls(true);
                          try {
                            await invoke('open_floating_controls', { sessionId: recordingState.sessionId });
                          } catch {
                            toast.error('Could not open floating controls. Try again from the tray.');
                          } finally {
                            setOpeningControls(false);
                          }
                        }}>
                        <PictureInPicture2 />
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>Open floating controls. Some screen-sharing apps may show this window.</TooltipContent>
                  </Tooltip>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <span className="inline-flex">
                        <LiveBookmarkButton sessionId={recordingState.sessionId} className={secondaryButtonClassName} disabled={isRecordingDisabled || isStopping || isPausing || isResuming} />
                      </span>
                    </TooltipTrigger>
                    <TooltipContent>
                      <p>{shortcutHint('Bookmark this moment', shortcuts.bookmarkRecording)}</p>
                    </TooltipContent>
                  </Tooltip>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <span className="inline-flex">
                        <LiveRecordingNotes key={recordingState.sessionId} sessionId={recordingState.sessionId} className={secondaryButtonClassName} disabled={isRecordingDisabled || isStopping || isPausing || isResuming} />
                      </span>
                    </TooltipTrigger>
                    <TooltipContent><p>Add timestamped note</p></TooltipContent>
                  </Tooltip>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button variant="ghost" size="icon"
                        aria-label={isPaused ? 'Resume recording' : 'Pause recording'}
                        onClick={() => {
                          if (isPaused) {
                            Analytics.trackButtonClick(
                              'resume_recording',
                              'recording_controls',
                            );
                            handleResumeRecording();
                          } else {
                            Analytics.trackButtonClick(
                              'pause_recording',
                              'recording_controls',
                            );
                            handlePauseRecording();
                          }
                        }}
                        disabled={isRecordingDisabled || isPausing || isResuming || isStopping}
                        className={secondaryButtonClassName}
                      >
                        {isPaused ? (
                          <Play size={16} />
                        ) : (
                          <Pause size={16} />
                        )}
                        {(isPausing || isResuming) && (
                          <div className="absolute -top-8 text-muted-foreground font-medium text-xs">
                            {isPausing ? 'Pausing...' : 'Resuming...'}
                          </div>
                        )}
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>
                      <p>
                        {shortcutHint(isPaused ? 'Resume recording' : 'Pause recording', shortcuts.pauseResume)}
                      </p>
                    </TooltipContent>
                  </Tooltip>

                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button variant="ghost" size="icon"
                        aria-label="Stop recording"
                        onClick={() => {
                          Analytics.trackButtonClick(
                            'stop_recording',
                            'recording_controls',
                          );
                          handleStopRecording();
                        }}
                        disabled={isRecordingDisabled || isStopping || isPausing || isResuming}
                        className={stopButtonClassName}
                      >
                        <Square size={16} />
                        {isStopping && (
                          <div className="absolute -top-8 text-muted-foreground font-medium text-xs">
                            Stopping...
                          </div>
                        )}
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>
                      <p>{shortcutHint('Stop recording', shortcuts.startStop)}</p>
                    </TooltipContent>
                  </Tooltip>
                </>
              )}

              {isRecording && <RecordingElapsedTime />}

              {/* Real levels while recording. Idle, three flat bars. */}
              {isRecording ? (
                <div className={isDashboard ? 'mx-3' : 'mx-4'}>
                  <RecordingLevelMeter paused={isPaused} />
                </div>
              ) : (
                <div className={waveformClassName} aria-hidden="true">
                  {[0, 1, 2].map((index) => (
                    <div
                      key={index}
                      className={`${isDashboard ? 'w-1.5' : 'w-1'} rounded-full bg-brand-gradient-v`}
                      style={{ height: isDashboard ? '8px' : '4px' }}
                    />
                  ))}
                </div>
              )}
            </>
          )}
        </div>

        {/* Show startup status. The backend may still be loading the speech model
            or opening audio devices, so keep the click visibly acknowledged. */}
        {isStartPending && (
          <div
            className={
              isDashboard
                ? 'mt-2 text-center text-xs text-primary/70'
                : 'text-xs text-muted-foreground text-center mt-2'
            }
          >
            Starting recording...
          </div>
        )}

        {/* Device error alert */}
        {deviceError && (
          <Alert
            variant="destructive"
            className="mt-4 border-error-border/30 bg-error"
          >
            <AlertCircle className="h-5 w-5 text-error-foreground" />
            <Button variant="ghost" size="icon"
              onClick={() => setDeviceError(null)}
              className="absolute right-3 top-3 h-auto w-auto p-0 hover:bg-transparent text-error-foreground hover:text-error-foreground transition-colors"
              aria-label="Close alert"
            >
              <X className="h-4 w-4" />
            </Button>
            <AlertTitle className="text-error-foreground font-semibold mb-2">
              {deviceError.title}
            </AlertTitle>
            <AlertDescription className="text-error-foreground">
              {deviceError.message.split('\n').map((line, i) => (
                <div key={i} className={i > 0 ? 'ml-2' : ''}>
                  {line}
                </div>
              ))}
            </AlertDescription>
          </Alert>
        )}
      </div>
    </TooltipProvider>
  );
};
