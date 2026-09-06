'use client';

import { invoke } from '@tauri-apps/api/core';
import { appDataDir } from '@tauri-apps/api/path';
import { useCallback, useEffect, useState, useRef } from 'react';
import { Play, Pause, Square, Mic, AlertCircle, X } from 'lucide-react';
import { ProcessRequest, SummaryResponse } from '@/types/summary';
import { listen } from '@tauri-apps/api/event';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import Analytics from '@/lib/analytics';
import { RecordingStatus, useRecordingState } from '@/contexts/RecordingStateContext';

interface RecordingControlsProps {
  isRecording: boolean;
  barHeights: string[];
  onRecordingStop: (callApi?: boolean) => void;
  onRecordingStart: () => void;
  onTranscriptReceived: (summary: SummaryResponse) => void;
  onTranscriptionError?: (message: string) => void;
  onStopInitiated?: () => void; // Called immediately when stop button is clicked
  isRecordingDisabled: boolean;
  isParentProcessing: boolean;
  selectedDevices?: {
    micDevice: string | null;
    systemDevice: string | null;
  };
  meetingName?: string;
  variant?: 'floating' | 'dashboard';
}

export const RecordingControls: React.FC<RecordingControlsProps> = ({
  isRecording,
  barHeights,
  onRecordingStop,
  onRecordingStart,
  onTranscriptReceived,
  onTranscriptionError,
  onStopInitiated,
  isRecordingDisabled,
  isParentProcessing,
  selectedDevices,
  meetingName,
  variant = 'floating',
}) => {
  // Use global recording state context for pause state (syncs with tray operations)
  const recordingState = useRecordingState();
  const isPaused = recordingState.isPaused;

  const [showPlayback, setShowPlayback] = useState(false);
  const [recordingPath, setRecordingPath] = useState<string | null>(null);
  const [transcript, setTranscript] = useState<string>('');
  const [isProcessing, setIsProcessing] = useState(false);
  const [isStarting, setIsStarting] = useState(false);
  const [isStopping, setIsStopping] = useState(false);
  const [isPausing, setIsPausing] = useState(false);
  const [isResuming, setIsResuming] = useState(false);
  const MIN_RECORDING_DURATION = 2000; // 2 seconds minimum recording time
  const [transcriptionErrors, setTranscriptionErrors] = useState(0);
  const [isValidatingModel, setIsValidatingModel] = useState(false);
  const [speechDetected, setSpeechDetected] = useState(false);
  const [deviceError, setDeviceError] = useState<{
    title: string;
    message: string;
  } | null>(null);
  const isStartPending =
    isStarting ||
    isValidatingModel ||
    recordingState.status === RecordingStatus.STARTING;

  // Playback scrubber values (unused until in-app playback exists).
  const currentTime = 0;
  const duration = 0;
  const isPlaying = false;
  const progress = 0;

  // Live elapsed clock: ticks every second while recording and not paused,
  // resets on stop. Display-only — the saved meeting's real duration comes from
  // the backend.
  const [elapsed, setElapsed] = useState(0);
  useEffect(() => {
    if (!isRecording) {
      setElapsed(0);
      return;
    }
    if (isPaused) return;
    const id = setInterval(() => setElapsed((e) => e + 1), 1000);
    return () => clearInterval(id);
  }, [isRecording, isPaused]);

  const formatTime = (time: number) => {
    const h = Math.floor(time / 3600);
    const m = Math.floor((time % 3600) / 60);
    const s = Math.floor(time % 60);
    const mm = m.toString().padStart(2, '0');
    const ss = s.toString().padStart(2, '0');
    return h > 0 ? `${h}:${mm}:${ss}` : `${m}:${ss}`;
  };

  useEffect(() => {
    const checkTauri = async () => {
      try {
        const result = await invoke('is_recording');
      } catch (error) {
        console.error("Tauri initialization error:");
        alert('Failed to initialize recording. Please check the console for details.');
      }
    };
    checkTauri();
  }, []);

  const handleStartRecording = useCallback(async () => {
    if (isStartPending) return;

    setShowPlayback(false);
    setTranscript(''); // Clear any previous transcript
    setSpeechDetected(false); // Reset speech detection on new recording
    setIsStarting(true);

    try {
      // Start through the shared lifecycle hook. The backend validates the
      // selected transcription engine before opening devices and emits any
      // model/download errors as structured events.
      await onRecordingStart();
    } catch (error) {
      console.error("Failed to start recording:");
      console.error("Error details:");

      // Parse error message to provide user-friendly feedback
      const errorMsg = error instanceof Error ? error.message : String(error);

      // Check for device-related errors
      if (
        errorMsg.includes('microphone') ||
        errorMsg.includes('mic') ||
        errorMsg.includes('input')
      ) {
        setDeviceError({
          title: 'Microphone Not Available',
          message:
            'Unable to access your microphone. Please check that:\n• Your microphone is connected\n• The app has microphone permissions\n• No other app is using the microphone'
        });
      } else if (
        errorMsg.includes('system audio') ||
        errorMsg.includes('speaker') ||
        errorMsg.includes('output')
      ) {
        setDeviceError({
          title: 'System Audio Not Available',
          message:
            'Unable to capture system audio. Please check that:\n• A virtual audio device (like BlackHole) is installed\n• The app has screen recording permissions (macOS)\n• System audio is properly configured'
        });
      } else if (errorMsg.includes('permission')) {
        setDeviceError({
          title: 'Permission Required',
          message:
            'Recording permissions are required. Please:\n• Grant microphone access in System Settings\n• Grant screen recording access for system audio (macOS)\n• Restart the app after granting permissions'
        });
      } else {
        setDeviceError({
          title: 'Recording Failed',
          message:
            'Unable to start recording. Please check your audio device settings and try again.'
        });
      }
    } finally {
      setIsStarting(false);
    }
  }, [
    onRecordingStart,
    isStartPending,
    selectedDevices,
    meetingName,
    isRecording,
  ]);

  const stopRecordingAction = useCallback(async () => {
    try {
      setIsProcessing(true);
      const dataDir = await appDataDir();
      const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
      const savePath = `${dataDir}/recording-${timestamp}.wav`;
      const result = await invoke('stop_recording', {
        args: {
          save_path: savePath,
        },
      });
      setRecordingPath(savePath);
      // setShowPlayback(true);
      setIsProcessing(false);
      // Track successful transcription
      Analytics.trackTranscriptionSuccess();
      onRecordingStop(true);
    } catch (error) {
      console.error("Failed to stop recording:");
      if (error instanceof Error) {
        console.error("Error details:");
        if (error.message.includes('No recording in progress')) {
          return;
        }
      } else if (
        typeof error === 'string' &&
        error.includes('No recording in progress')
      ) {
        return;
      } else if (error && typeof error === 'object' && 'toString' in error) {
        if (error.toString().includes('No recording in progress')) {
          return;
        }
      }
      setIsProcessing(false);
      onRecordingStop(false);
    } finally {
      setIsStopping(false);
    }
  }, [onRecordingStop]);

  const handleStopRecording = useCallback(async () => {
    if (!isRecording || isStarting || isStopping) {
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
    stopRecordingAction,
    onStopInitiated,
  ]);

  const handlePauseRecording = useCallback(async () => {
    if (!isRecording || isPaused || isPausing) return;

    setIsPausing(true);

    try {
      await invoke('pause_recording');
      // isPaused state now managed by RecordingStateContext via events
    } catch (error) {
      console.error("Failed to pause recording:");
      alert('Failed to pause recording. Please check the console for details.');
    } finally {
      setIsPausing(false);
    }
  }, [isRecording, isPaused, isPausing]);

  const handleResumeRecording = useCallback(async () => {
    if (!isRecording || !isPaused || isResuming) return;

    setIsResuming(true);

    try {
      await invoke('resume_recording');
      // isPaused state now managed by RecordingStateContext via events
    } catch (error) {
      console.error("Failed to resume recording:");
      alert('Failed to resume recording. Please check the console for details.');
    } finally {
      setIsResuming(false);
    }
  }, [isRecording, isPaused, isResuming]);

  useEffect(() => {
    return () => {
      // Cleanup on unmount if needed
    };
  }, []);

  useEffect(() => {
    let unsubscribes: (() => void)[] = [];

    const setupListeners = async () => {
      try {
        // Transcript error listener - handles both regular and actionable errors
        const transcriptErrorUnsubscribe = await listen(
          'transcript-error',
          (event) => {
            console.error("Transcription error received:");
            const errorMessage = event.payload as string;

            Analytics.trackTranscriptionError(errorMessage);

            setTranscriptionErrors((prev) => {
              const newCount = prev + 1;
              return newCount;
            });
            setIsProcessing(false);
            if (onTranscriptionError) {
              onTranscriptionError(errorMessage);
            }
          },
        );

        // Transcription error listener - handles structured error objects with actionable flag
        const transcriptionErrorUnsubscribe = await listen(
          'transcription-error',
          (event) => {
            console.error("Transcription error received:");

            let errorMessage: string;
            let isActionable = false;

            if (typeof event.payload === 'object' && event.payload !== null) {
              const payload = event.payload as {
                error: string;
                userMessage: string;
                actionable: boolean;
              };
              errorMessage = payload.userMessage || payload.error;
              isActionable = payload.actionable || false;
            } else {
              errorMessage = String(event.payload);
            }

            Analytics.trackTranscriptionError(errorMessage);

            setTranscriptionErrors((prev) => {
              const newCount = prev + 1;
              return newCount;
            });
            setIsProcessing(false);

            // For actionable errors (like model loading failures), the main page will handle showing the model selector
            // For regular errors, they are handled by useModalState global listener which shows a toast
            // We don't want to show a modal (via onTranscriptionError) AND a toast, so we skip the callback here
            /* if (onTranscriptionError && !isActionable) {
            onTranscriptionError(errorMessage);
          } */
          },
        );

        // Pause/Resume events are now handled by RecordingStateContext
        // No need for duplicate listeners here

        // Speech detected listener - for UX feedback when VAD detects speech
        const speechDetectedUnsubscribe = await listen(
          'speech-detected',
          (event) => {
            setSpeechDetected(true);
          },
        );

        unsubscribes = [
          transcriptErrorUnsubscribe,
          transcriptionErrorUnsubscribe,
          speechDetectedUnsubscribe,
        ];
      } catch (error) {
        console.error("Failed to set up recording event listeners:");
      }
    };

    setupListeners();

    return () => {
      unsubscribes.forEach((unsubscribe) => {
        if (unsubscribe && typeof unsubscribe === 'function') {
          unsubscribe();
        }
      });
    };
  }, [onRecordingStop, onTranscriptionError]);

  const isDashboard = variant === 'dashboard';
  const shellClassName = isDashboard
    ? 'flex flex-col space-y-2'
    : 'flex flex-col space-y-2';
  const controlsClassName = isDashboard
    ? 'flex items-center gap-3 bg-transparent p-0 shadow-none'
    : 'flex items-center space-x-2 bg-card rounded-full shadow-sm px-4 py-2';
  const startButtonClassName = isDashboard
    ? `h-16 w-16 flex items-center justify-center rounded-full text-primary-foreground shadow-[0_0_34px_hsl(var(--primary)/0.42)] transition-all relative ${isStartPending || isProcessing || isRecordingDisabled ? 'bg-muted cursor-not-allowed' : 'bg-gradient-to-br from-primary to-primary/70 hover:scale-105 hover:shadow-[0_0_44px_hsl(var(--primary)/0.6)]'}`
    : `w-12 h-12 flex items-center justify-center ${isStartPending || isProcessing ? 'bg-muted' : 'bg-red-500 hover:bg-red-600'} rounded-full text-foreground transition-colors relative`;
  const secondaryButtonClassName = isDashboard
    ? `h-12 w-12 flex items-center justify-center rounded-full border border-border text-foreground transition-colors relative ${isPausing || isResuming || isStopping ? 'bg-muted text-muted-foreground' : 'bg-muted hover:bg-accent'}`
    : `w-10 h-10 flex items-center justify-center ${isPausing || isResuming || isStopping ? 'bg-muted border-2 border-border text-muted-foreground' : 'bg-card border-2 border-border text-muted-foreground hover:border-ring hover:bg-muted'} rounded-full transition-colors relative`;
  const stopButtonClassName = isDashboard
    ? `h-12 w-12 flex items-center justify-center rounded-full text-foreground transition-colors relative ${isStopping || isPausing || isResuming ? 'bg-muted cursor-not-allowed' : 'bg-red-500 hover:bg-red-400'}`
    : `w-10 h-10 flex items-center justify-center ${isStopping || isPausing || isResuming ? 'bg-muted' : 'bg-red-500 hover:bg-red-600'} rounded-full text-foreground transition-colors relative`;
  const waveformClassName = isDashboard
    ? 'flex items-center justify-center gap-1.5 rounded-full border border-primary/30 bg-primary/10 px-4 py-3'
    : 'flex items-center space-x-1 mx-4';

  return (
    <TooltipProvider>
      <div className={shellClassName}>
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
              <span
                className={
                  isDashboard
                    ? 'text-sm text-muted-foreground'
                    : 'text-sm text-muted-foreground'
                }
              >
                Processing recording...
              </span>
            </div>
          ) : (
            <>
              {showPlayback ? (
                <>
                  <button
                    onClick={handleStartRecording}
                    className="w-10 h-10 flex items-center justify-center bg-red-500 rounded-full text-foreground hover:bg-red-600 transition-colors"
                  >
                    <Mic size={16} />
                  </button>

                  <div className="w-px h-6 bg-border mx-1" />

                  <div className="flex items-center space-x-1 mx-2">
                    <div className="text-sm text-muted-foreground min-w-[40px]">
                      {formatTime(currentTime)}
                    </div>
                    <div className="relative w-24 h-1 bg-muted rounded-full">
                      <div
                        className="absolute h-full bg-primary rounded-full"
                        style={{ width: `${progress}%` }}
                      />
                    </div>
                    <div className="text-sm text-muted-foreground min-w-[40px]">
                      {formatTime(duration)}
                    </div>
                  </div>

                  <button
                    className="w-10 h-10 flex items-center justify-center bg-secondary rounded-full text-foreground cursor-not-allowed"
                    disabled
                  >
                    <Play size={16} />
                  </button>
                </>
              ) : (
                <>
                  {!isRecording ? (
                    // Start recording button
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <button
                          onClick={() => {
                            Analytics.trackButtonClick(
                              'start_recording',
                              'recording_controls',
                            );
                            handleStartRecording();
                          }}
                          disabled={
                            isStarting ||
                            isProcessing ||
                            isRecordingDisabled ||
                            isStartPending
                          }
                          className={startButtonClassName}
                        >
                          {isStartPending ? (
                            <div className="animate-spin rounded-full h-5 w-5 border-b-2 border-white"></div>
                          ) : (
                            <Mic size={isDashboard ? 26 : 20} />
                          )}
                        </button>
                      </TooltipTrigger>
                      <TooltipContent>
                        <p>{isStartPending ? 'Starting recording...' : 'Start recording'}</p>
                      </TooltipContent>
                    </Tooltip>
                  ) : (
                    // Recording controls (pause/resume + stop)
                    <>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
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
                            disabled={isPausing || isResuming || isStopping}
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
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>
                          <p>
                            {isPaused ? 'Resume recording' : 'Pause recording'}
                          </p>
                        </TooltipContent>
                      </Tooltip>

                      <Tooltip>
                        <TooltipTrigger asChild>
                          <button
                            onClick={() => {
                              Analytics.trackButtonClick(
                                'stop_recording',
                                'recording_controls',
                              );
                              handleStopRecording();
                            }}
                            disabled={isStopping || isPausing || isResuming}
                            className={stopButtonClassName}
                          >
                            <Square size={16} />
                            {isStopping && (
                              <div className="absolute -top-8 text-muted-foreground font-medium text-xs">
                                Stopping...
                              </div>
                            )}
                          </button>
                        </TooltipTrigger>
                        <TooltipContent>
                          <p>Stop recording</p>
                        </TooltipContent>
                      </Tooltip>
                    </>
                  )}

                  {isRecording && (
                    <span className="font-mono text-sm tabular-nums text-foreground min-w-[3.25rem] text-center">
                      {formatTime(elapsed)}
                    </span>
                  )}

                  <div className={waveformClassName}>
                    {barHeights.map((height, index) => (
                      <div
                        key={index}
                        className={`${isDashboard ? 'w-1.5' : 'w-1'} rounded-full transition-all duration-200 ${
                          isPaused ? 'bg-orange-400' : 'bg-brand-gradient-v'
                        }`}
                        style={{
                          height:
                            isRecording && !isPaused
                              ? height
                              : isDashboard
                                ? '8px'
                                : '4px',
                          opacity: isPaused ? 0.6 : 1,
                        }}
                      />
                    ))}
                  </div>
                </>
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
            className="mt-4 border-red-300 bg-red-50"
          >
            <AlertCircle className="h-5 w-5 text-red-600" />
            <button
              onClick={() => setDeviceError(null)}
              className="absolute right-3 top-3 text-red-600 hover:text-red-800 transition-colors"
              aria-label="Close alert"
            >
              <X className="h-4 w-4" />
            </button>
            <AlertTitle className="text-red-800 font-semibold mb-2">
              {deviceError.title}
            </AlertTitle>
            <AlertDescription className="text-red-700">
              {deviceError.message.split('\n').map((line, i) => (
                <div key={i} className={i > 0 ? 'ml-2' : ''}>
                  {line}
                </div>
              ))}
            </AlertDescription>
          </Alert>
        )}

        {/* {showPlayback && recordingPath && (
        <div className="text-sm text-muted-foreground px-4">
          Recording saved to: {recordingPath}
        </div>
      )} */}
      </div>
    </TooltipProvider>
  );
};
