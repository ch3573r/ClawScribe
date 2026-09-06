import { useState, useEffect, useCallback, useRef } from 'react';
import { useRouter } from 'next/navigation';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import type { RecordingStoppedPayload } from '@/services/recordingService';
import { recordingOutcome, recordingRecoveryMessage } from '@/lib/recording-outcome';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';
import { storageService } from '@/services/storageService';
import { transcriptService } from '@/services/transcriptService';
import Analytics from '@/lib/analytics';
import { takeActiveRecordingCalendar, setMeetingCalendar } from '@/lib/meetingCalendar';
import {
  applyPinnedSummaryLanguageToMeeting,
  detectAndCacheSummaryLanguage,
} from '@/lib/summary-language-preferences';

type SummaryStatus = 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error';

interface UseRecordingStopReturn {
  handleRecordingStop: (callApi: boolean) => Promise<void>;
  isStopping: boolean;
  isProcessingTranscript: boolean;
  isSavingTranscript: boolean;
  summaryStatus: SummaryStatus;
  setIsStopping: (value: boolean) => void;
}

/**
 * Custom hook for managing recording stop lifecycle.
 * Handles the complex stop sequence: transcription wait → buffer flush → SQLite save → navigation.
 *
 * Features:
 * - Transcription completion polling (60s max, 500ms interval)
 * - Transcript buffer flush coordination
 * - SQLite meeting save with folder_path from sessionStorage
 * - Comprehensive analytics tracking (duration, word count, activation)
 * - Auto-navigation to meeting details
 * - Toast notifications for success/error
 * - Window exposure for Rust callbacks
 */
export function useRecordingStop(
  setIsRecording: (value: boolean) => void,
  setIsRecordingDisabled: (value: boolean) => void
): UseRecordingStopReturn {
  // USE global state instead
  const recordingState = useRecordingState();
  const {
    status,
    setStatus,
    isStopping,
    isProcessing: isProcessingTranscript,
    isSaving: isSavingTranscript
  } = recordingState;

  const {
    transcriptsRef,
    flushBuffer,
    clearTranscripts,
    meetingTitle,
    markMeetingAsSaved,
  } = useTranscripts();

  const {
    refetchMeetings,
    setCurrentMeeting,
    setMeetings,
    meetings,
    setIsMeetingActive,
  } = useSidebar();

  const router = useRouter();

  // Guard to prevent duplicate/concurrent stop calls (e.g., from UI and tray simultaneously)
  const stopInProgressRef = useRef(false);

  // Promise to track recording-stopped event data (fixes race condition with recording-stop-complete)
  const recordingStoppedDataRef = useRef<Promise<void> | null>(null);

  // Set up recording-stopped listener for meeting navigation
  useEffect(() => {
    let unlistenFn: (() => void) | undefined;
    let disposed = false;

    const setupRecordingStoppedListener = async () => {
      try {
        unlistenFn = await listen<RecordingStoppedPayload>('recording-stopped', async (event) => {
          if (disposed) return;
          // Create promise that resolves when sessionStorage is set (prevents race condition)
          recordingStoppedDataRef.current = (async () => {
            const { folder_path, meeting_name } = event.payload;
            sessionStorage.setItem('last_recording_mode', event.payload.recording_mode ?? 'live');
            sessionStorage.setItem('last_recording_outcome', JSON.stringify(recordingOutcome(event.payload)));

            // Store folder_path and meeting_name for later use in handleRecordingStop
            if (folder_path) {
              sessionStorage.setItem('last_recording_folder_path', folder_path);
            }
            if (meeting_name) {
              sessionStorage.setItem('last_recording_meeting_name', meeting_name);
            }
          })();

        });
        if (disposed) unlistenFn();
      } catch (error) {
        console.error("Failed to setup recording stopped listener:");
      }
    };

    setupRecordingStoppedListener();

    return () => {
      disposed = true;
      if (unlistenFn) {
        unlistenFn();
      }
    };
  }, [router]);

  // Main recording stop handler
  const handleRecordingStop = useCallback(async (isCallApi: boolean) => {
    if (recordingStoppedDataRef.current) {
      await recordingStoppedDataRef.current;
    }

    // Guard: prevent duplicate/concurrent stop calls
    if (stopInProgressRef.current) {
      return;
    }
    stopInProgressRef.current = true;

    // Set status to STOPPING immediately
    setStatus(RecordingStatus.STOPPING);
    setIsRecording(false);
    setIsRecordingDisabled(true);
    const stopStartTime = Date.now();

    try {

      // Note: stop_recording is already called by RecordingControls.stopRecordingAction
      // This function only handles post-stop processing (transcription wait, API call, navigation)

      // Wait for transcription to complete
      const audioOnly = sessionStorage.getItem('last_recording_mode') === 'audio_only';
      if (!audioOnly) setStatus(RecordingStatus.PROCESSING_TRANSCRIPTS, 'Waiting for transcription...');

      const MAX_WAIT_TIME = 60000; // 60 seconds maximum wait (increased for longer processing)
      const POLL_INTERVAL = 500; // Check every 500ms
      let elapsedTime = 0;
      let transcriptionComplete = audioOnly;

      // Listen for transcription-complete event
      const unlistenComplete = await listen<{failed?: boolean; cancelled?: boolean; chunks_remaining?: number}>('transcription-complete', (event) => {
        if (event.payload.failed || event.payload.cancelled || (event.payload.chunks_remaining ?? 0) > 0) {
          const outcome = recordingOutcome(JSON.parse(sessionStorage.getItem('last_recording_outcome') || '{}'));
          outcome.transcription_incomplete = true;
          sessionStorage.setItem('last_recording_outcome', JSON.stringify(outcome));
        }
        transcriptionComplete = true;
      });

      // Poll for transcription status
      while (elapsedTime < MAX_WAIT_TIME && !transcriptionComplete) {
        try {
          const status = await transcriptService.getTranscriptionStatus();

          // Check if transcription is complete
          if (!status.is_processing && status.chunks_in_queue === 0) {
            transcriptionComplete = true;
            break;
          }

          // The backend owns the drain and explicitly stops a worker after its
          // safety deadline. Do not wait another minute when no worker exists;
          // the original audio remains available for retranscription.
          if (!status.is_processing && status.chunks_in_queue > 0) {
            console.warn("Transcription worker stopped with queued audio remaining");
            setStatus(
              RecordingStatus.PROCESSING_TRANSCRIPTS,
              `Live transcript incomplete (${status.chunks_in_queue} chunks); audio preserved`
            );
            transcriptionComplete = true;
            break;
          }

          // If no activity for more than 8 seconds and no chunks in queue, consider it done (increased from 5s to 8s)
          if (status.last_activity_ms > 8000 && status.chunks_in_queue === 0) {
            transcriptionComplete = true;
            break;
          }

          // Update user with current status
          if (status.chunks_in_queue > 0) {
            const eta = status.estimated_seconds_remaining != null
              ? ` (~${Math.ceil(status.estimated_seconds_remaining)}s)`
              : '';
            setStatus(
              RecordingStatus.PROCESSING_TRANSCRIPTS,
              `Processing ${status.chunks_in_queue} remaining chunks${eta}...`
            );
          }

          // Wait before next check
          await new Promise(resolve => setTimeout(resolve, POLL_INTERVAL));
          elapsedTime += POLL_INTERVAL;
        } catch (error) {
          console.error("Error checking transcription status:");
          break;
        }
      }

      // Clean up listener
      unlistenComplete();

      if (!transcriptionComplete && elapsedTime >= MAX_WAIT_TIME) {
        console.warn("⏰ Transcription wait timeout reached after");
      } else {
        // Rust has already joined the worker before stop_recording returns; only
        // allow one UI tick for the final event handlers to settle.
        await new Promise(resolve => setTimeout(resolve, 250));
      }

      // Final buffer flush: process ALL remaining transcripts regardless of timing
      const flushStartTime = Date.now();
      if (!audioOnly) setStatus(RecordingStatus.PROCESSING_TRANSCRIPTS, 'Flushing transcript buffer...');
      flushBuffer();
      const flushEndTime = Date.now();

      // NOTE: Status remains PROCESSING_TRANSCRIPTS until we start saving

      // Wait a bit more to ensure all transcript state updates have been processed
      await new Promise(resolve => setTimeout(resolve, 500));

      // Save to SQLite
      // NOTE: enabled to save COMPLETE transcripts after frontend receives all updates
      // This ensures user sees all transcripts streaming in before database save
      if (isCallApi && transcriptionComplete == true) {

        setStatus(RecordingStatus.SAVING, 'Saving meeting to database...');

        // Get fresh transcript state (ALL transcripts including late ones)
        const freshTranscripts = [...transcriptsRef.current];

        // Get folder_path and meeting_name from recording-stopped event
        const folderPath = sessionStorage.getItem('last_recording_folder_path');
        const savedMeetingName = sessionStorage.getItem('last_recording_meeting_name');


        try {
          const responseData = await storageService.saveMeeting(
            savedMeetingName || meetingTitle || 'New Meeting',  // PREFER savedMeetingName (backend source)
            freshTranscripts,
            folderPath,
            recordingOutcome(JSON.parse(sessionStorage.getItem('last_recording_outcome') || '{}')),
          );

          const meetingId = responseData.meeting_id;
          if (!meetingId) {
            console.error("No meeting_id in response:");
            throw new Error('No meeting ID received from save operation');
          }

          // Bind the calendar event that was frozen at THIS recording's start
          // (consumed here) to the saved meeting id. Using the active snapshot —
          // not the live pending value — means a "Use for next recording" change
          // made mid-recording can't rebind this meeting.
          try {
            const activeCal = takeActiveRecordingCalendar();
            if (activeCal) {
              setMeetingCalendar(meetingId, activeCal);
            }
          } catch (error) {
            console.warn("Failed to bind calendar selection to meeting:");
          }

          let shouldDetectSummaryLanguage = false;
          try {
            shouldDetectSummaryLanguage = !(await applyPinnedSummaryLanguageToMeeting(meetingId));
          } catch (error) {
            console.warn("Failed to apply pinned summary language preference for new meeting:");
            toast.warning('Could not apply default summary language', {
              description: 'The meeting was saved, but the default summary language was not applied.',
            });
          }

          if (shouldDetectSummaryLanguage) {
            try {
              await detectAndCacheSummaryLanguage(
                meetingId,
                freshTranscripts.map(t => t.text)
              );
            } catch (error) {
              console.warn("Failed to detect summary language for new meeting:");
              toast.warning('Could not detect summary language', {
                description: 'The meeting was saved, but Auto could not detect the summary language.',
              });
            }
          }


          // Mark meeting as saved in IndexedDB (for recovery system)
          await markMeetingAsSaved();

          // Clean up session storage
          sessionStorage.removeItem('last_recording_folder_path');
          sessionStorage.removeItem('last_recording_meeting_name');
          // Clean up IndexedDB meeting ID (redundant with markMeetingAsSaved cleanup, but ensures cleanup)
          sessionStorage.removeItem('indexeddb_current_meeting_id');

          // Refetch meetings and set current meeting
          await refetchMeetings();

          try {
            const meetingData = await storageService.getMeeting(meetingId);
            if (meetingData) {
              setCurrentMeeting({
                id: meetingId,
                title: meetingData.title
              });
            }
          } catch (error) {
            console.warn("Could not fetch meeting details, using ID only:");
            setCurrentMeeting({ id: meetingId, title: savedMeetingName || meetingTitle || 'New Meeting' });
          }

          // Mark as completed
          setStatus(RecordingStatus.COMPLETED);

          // Show success toast with navigation option
          const recoveryMessage = recordingRecoveryMessage(recordingOutcome(JSON.parse(sessionStorage.getItem('last_recording_outcome') || '{}')));
          (recoveryMessage ? toast.warning : toast.success)(recoveryMessage ? 'Meeting saved with recovery needed' : 'Recording saved successfully!', {
            description: recoveryMessage || (audioOnly ? 'Audio saved. Open the meeting and choose Transcribe when ready.' : `${freshTranscripts.length} transcript segments saved.`),
            action: {
              label: 'View Meeting',
              onClick: () => {
                router.push(`/meeting-details?id=${meetingId}`);
                Analytics.trackButtonClick('view_meeting_from_toast', 'recording_complete');
              }
            },
            duration: 10000,
          });

          // Auto-navigate after a short delay with source parameter
          setTimeout(() => {
            router.push(`/meeting-details?id=${meetingId}&source=recording`);
            clearTranscripts()
            Analytics.trackPageView('meeting_details');

            // Reset to IDLE after navigation
            setStatus(RecordingStatus.IDLE);
          }, 2000);
          // Track meeting completion analytics
          try {
            // Calculate meeting duration from transcript timestamps
            let durationSeconds = 0;
            if (freshTranscripts.length > 0 && freshTranscripts[0].audio_start_time !== undefined) {
              // Use audio_end_time of last transcript if available
              const lastTranscript = freshTranscripts[freshTranscripts.length - 1];
              durationSeconds = lastTranscript.audio_end_time || lastTranscript.audio_start_time || 0;
            }

            // Calculate word count
            const transcriptWordCount = freshTranscripts
              .map(t => t.text.split(/\s+/).length)
              .reduce((a, b) => a + b, 0);

            // Calculate words per minute
            const wordsPerMinute = durationSeconds > 0 ? transcriptWordCount / (durationSeconds / 60) : 0;

            // Get meetings count today
            const meetingsToday = await Analytics.getMeetingsCountToday();

            // Track meeting completed
            await Analytics.trackMeetingCompleted(meetingId, {
              duration_seconds: durationSeconds,
              transcript_segments: freshTranscripts.length,
              transcript_word_count: transcriptWordCount,
              words_per_minute: wordsPerMinute,
              meetings_today: meetingsToday
            });

            // Update meeting count in analytics.json
            await Analytics.updateMeetingCount();

            // Check for activation (first meeting)
            const { Store } = await import('@tauri-apps/plugin-store');
            const store = await Store.load('analytics.json');
            const totalMeetings = await store.get<number>('total_meetings');

            if (totalMeetings === 1) {
              const daysSinceInstall = await Analytics.calculateDaysSince('first_launch_date');
              await Analytics.track('user_activated', {
                meetings_count: '1',
                days_since_install: daysSinceInstall?.toString() || 'null',
                first_meeting_duration_seconds: durationSeconds.toString()
              });
            }
          } catch (analyticsError) {
            console.error("Failed to track meeting completion analytics:");
            // Don't block user flow on analytics errors
          }

        } catch (saveError) {
          console.error("Failed to save meeting to database:");
          setStatus(RecordingStatus.ERROR, saveError instanceof Error ? saveError.message : 'Unknown error');
          toast.error('Failed to save meeting', {
            description: saveError instanceof Error ? saveError.message : 'Unknown error'
          });
          throw saveError;
        }
      } else {
        // No save needed, go back to IDLE
        setStatus(RecordingStatus.IDLE);
      }

      setIsMeetingActive(false);
      // isRecording already set to false at function start
      setIsRecordingDisabled(false);
    } catch (error) {
      console.error("Error in handleRecordingStop:");
      setStatus(RecordingStatus.ERROR, error instanceof Error ? error.message : 'Unknown error');
      // isRecording already set to false at function start
      setIsRecordingDisabled(false);
    } finally {
      // Always reset the guard flag when done
      stopInProgressRef.current = false;
    }
  }, [
    setIsRecording,
    setIsRecordingDisabled,
    setStatus,
    transcriptsRef,
    flushBuffer,
    clearTranscripts,
    meetingTitle,
    markMeetingAsSaved,
    refetchMeetings,
    setCurrentMeeting,
    setMeetings,
    meetings,
    setIsMeetingActive,
    router,
  ]);

  // Expose handleRecordingStop function to window for Rust callbacks
  const handleRecordingStopRef = useRef(handleRecordingStop);
  useEffect(() => {
    handleRecordingStopRef.current = handleRecordingStop;
  });

  useEffect(() => {
    (window as any).handleRecordingStop = (callApi: boolean = true) => {
      handleRecordingStopRef.current(callApi);
    };

    // Cleanup on unmount
    return () => {
      delete (window as any).handleRecordingStop;
    };
  }, []);

  // Derive summaryStatus from RecordingStatus for backward compatibility
  const summaryStatus: SummaryStatus = status === RecordingStatus.PROCESSING_TRANSCRIPTS ? 'processing' : 'idle';

  return {
    handleRecordingStop,
    isStopping,
    isProcessingTranscript,
    isSavingTranscript,
    summaryStatus,
    setIsStopping: (value: boolean) => {
      setStatus(value ? RecordingStatus.STOPPING : RecordingStatus.IDLE);
    },
  };
}
