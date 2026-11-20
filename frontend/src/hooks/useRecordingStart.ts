import { useState, useEffect, useCallback } from 'react';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { useConfig } from '@/contexts/ConfigContext';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';
import { recordingService } from '@/services/recordingService';
import Analytics from '@/lib/analytics';
import { showRecordingNotification } from '@/lib/recordingNotification';
import { offerSpeakerDetectionCancellation } from '@/lib/speakerDetectionCancellation';
import { getNextCalendarChoice, beginRecordingCalendar, microsoftCalendarRevision, type RecordingCalendar } from '@/lib/meetingCalendar';
import { findOngoingRecordingCalendar } from '@/services/recordingCalendarService';
import { toast } from 'sonner';
import { isDiskSpaceRefusal, startErrorText } from '@/lib/start-error';

interface UseRecordingStartReturn {
  handleRecordingStart: () => Promise<void>;
  isAutoStarting: boolean;
}

function isTranscriptionReadinessError(error: unknown): boolean {
  const message = (error instanceof Error ? error.message : String(error)).toLowerCase();
  return (
    message.includes('transcription') ||
    message.includes('speech recognition') ||
    message.includes('speaker detection') ||
    message.includes('model') ||
    message.includes('provider')
  );
}

/**
 * Tells the user why a start from the sidebar or the tray failed. A refusal for
 * a nearly full disk keeps its whole message; engine problems are explained by
 * their own dialogs; anything else gets the general hint.
 */
function reportStartFailure(error: unknown): void {
  if (isDiskSpaceRefusal(error)) {
    toast.error('Not enough disk space', { description: startErrorText(error), duration: 15000 });
  } else if (!isTranscriptionReadinessError(error)) {
    toast.error('Could not start recording. Check your audio devices and microphone permissions, then try again.');
  }
}

/**
 * Custom hook for managing recording start lifecycle.
 * Handles both manual start (button click) and auto-start (from sidebar navigation).
 *
 * Features:
 * - Meeting title generation (format: Meeting DD_MM_YY_HH_MM_SS)
 * - Transcript clearing on start
 * - Analytics tracking
 * - Recording notification display
 * - Auto-start from sidebar via sessionStorage flag
 */
export function useStartRecordingSession(setIsRecording: (value: boolean) => void) {
  const { clearTranscripts, setMeetingTitle } = useTranscripts();
  const { setIsMeetingActive } = useSidebar();
  const { selectedDevices } = useConfig();
  const { setStatus } = useRecordingState();

  // Generate meeting title with timestamp
  const generateMeetingTitle = useCallback(() => {
    const now = new Date();
    const day = String(now.getDate()).padStart(2, '0');
    const month = String(now.getMonth() + 1).padStart(2, '0');
    const year = String(now.getFullYear()).slice(-2);
    const hours = String(now.getHours()).padStart(2, '0');
    const minutes = String(now.getMinutes()).padStart(2, '0');
    const seconds = String(now.getSeconds()).padStart(2, '0');
    return `Meeting ${day}_${month}_${year}_${hours}_${minutes}_${seconds}`;
  }, []);

  const startBackendRecording = useCallback(
    async (source: 'home_page' | 'sidebar_auto' | 'sidebar_direct' | 'detected_call'): Promise<{ title: string; sessionId?: string }> => {
      setStatus(RecordingStatus.STARTING, 'Initializing recording...');
      const now = Date.now();
      const choice = getNextCalendarChoice();
      const microsoftRevision = microsoftCalendarRevision;
      const fallbackTitle = generateMeetingTitle();
      const match = choice.link || choice.skip ? { status: "none" as const, matches: [] }
        : await findOngoingRecordingCalendar(now).catch(() => ({ status: "unavailable" as const, matches: [] }));
      const active: RecordingCalendar = {
        ...match, fallbackTitle,
        link: choice.link ?? (match.status === "matched" ? match.matches[0] : null),
      };
      const nativeTitle = choice.link?.subject?.trim() || fallbackTitle;

      let sessionId: string | undefined;
      const mic = selectedDevices?.micDevice || null;
      const system = selectedDevices?.systemDevice || null;
      try {
        if (source === 'detected_call') {
          sessionId = await recordingService.startDetectedCall(mic, system, nativeTitle);
        } else {
          await recordingService.startRecordingWithDevices(mic, system, nativeTitle);
        }
      } catch (error) {
        // A delayed retry of a detected start needs fresh meeting evidence.
        if (source !== 'detected_call') await offerSpeakerDetectionCancellation(error, () => startBackendRecording(source));
        throw error;
      }

      if (microsoftCalendarRevision !== microsoftRevision) {
        if (active.link?.source === "microsoft") active.link = null;
        active.matches = active.matches.filter(link => link.source !== "microsoft");
      }
      const title = active.link?.subject?.trim() || fallbackTitle;
      beginRecordingCalendar(active, choice);
      setMeetingTitle(title);
      setIsRecording(true);
      clearTranscripts();
      setIsMeetingActive(true);
      Analytics.trackButtonClick('start_recording', source);
      void showRecordingNotification();
      return { title, sessionId };
    },
    [
      clearTranscripts,
      selectedDevices,
      setIsMeetingActive,
      setIsRecording,
      setMeetingTitle,
      setStatus,
      generateMeetingTitle,
    ],
  );
  return startBackendRecording;
}

export function useRecordingStart(
  isRecording: boolean,
  setIsRecording: (value: boolean) => void
): UseRecordingStartReturn {
  const [isAutoStarting, setIsAutoStarting] = useState(false);
  const startBackendRecording = useStartRecordingSession(setIsRecording);
  const { setStatus } = useRecordingState();

  // Handle manual recording start (from button click)
  const handleRecordingStart = useCallback(async () => {
    try {
      console.log('handleRecordingStart called - starting backend recording');
      await startBackendRecording('home_page');
      console.log('Backend recording started successfully');
    } catch (error) {
      console.error('Failed to start recording:', error);
      setStatus(RecordingStatus.ERROR, error instanceof Error ? error.message : 'Failed to start recording');
      setIsRecording(false); // Reset state on error
      Analytics.trackButtonClick('start_recording_error', 'home_page');
      // The Start button shows a refused start (full disk) in its own panel.
      if (!isDiskSpaceRefusal(error) && isTranscriptionReadinessError(error)) return;
      // Re-throw so RecordingControls can handle device-specific errors
      throw error;
    }
  }, [setIsRecording, setStatus, startBackendRecording]);

  // Check for autoStartRecording flag and start recording automatically
  useEffect(() => {
    const checkAutoStartRecording = async () => {
      if (typeof window !== 'undefined') {
        const shouldAutoStart = sessionStorage.getItem('autoStartRecording');
        if (shouldAutoStart === 'true' && !isRecording && !isAutoStarting) {
          console.log('Auto-starting recording from navigation...');
          setIsAutoStarting(true);
          sessionStorage.removeItem('autoStartRecording'); // Clear the flag

          // Start the actual backend recording
          try {
            await startBackendRecording('sidebar_auto');
          } catch (error) {
            console.error('Failed to auto-start recording:', error);
            setStatus(RecordingStatus.ERROR, error instanceof Error ? error.message : 'Failed to auto-start recording');
            reportStartFailure(error);
            Analytics.trackButtonClick('start_recording_error', 'sidebar_auto');
          } finally {
            setIsAutoStarting(false);
          }
        }
      }
    };

    checkAutoStartRecording();
  }, [
    isRecording,
    isAutoStarting,
    setStatus,
    startBackendRecording,
  ]);

  // Listen for direct recording trigger from sidebar when already on home page
  useEffect(() => {
    const handleDirectStart = async () => {
      if (isRecording || isAutoStarting) {
        console.log('Recording already in progress, ignoring direct start event');
        return;
      }

      console.log('Direct start from sidebar - starting backend recording');
      setIsAutoStarting(true);

      try {
        await startBackendRecording('sidebar_direct');
      } catch (error) {
        console.error('Failed to start recording from sidebar:', error);
        setStatus(RecordingStatus.ERROR, error instanceof Error ? error.message : 'Failed to start recording from sidebar');
        reportStartFailure(error);
        Analytics.trackButtonClick('start_recording_error', 'sidebar_direct');
      } finally {
        setIsAutoStarting(false);
      }
    };

    window.addEventListener('start-recording-from-sidebar', handleDirectStart);

    return () => {
      window.removeEventListener('start-recording-from-sidebar', handleDirectStart);
    };
  }, [
    isRecording,
    isAutoStarting,
    setStatus,
    startBackendRecording,
  ]);

  return {
    handleRecordingStart,
    isAutoStarting,
  };
}
