import { useState, useEffect, useCallback } from 'react';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { useConfig } from '@/contexts/ConfigContext';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';
import { recordingService } from '@/services/recordingService';
import Analytics from '@/lib/analytics';
import { showRecordingNotification } from '@/lib/recordingNotification';
import { offerSpeakerDetectionCancellation } from '@/lib/speakerDetectionCancellation';
import { getPendingCalendar, beginRecordingCalendar } from '@/lib/meetingCalendar';
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

  // Title every recording (home button, sidebar auto/direct) from the calendar
  // event chosen for the next recording, if any; else the generated title.
  // Read the pending calendar selection ONCE before starting, and derive the
  // title from that same object — so the title and the attendee binding can't
  // diverge if "Use for next recording" changes during the async startup. The
  // returned `calendar` is frozen via beginRecordingCalendar(calendar) on success.
  const snapshotPendingForStart = useCallback(() => {
    const calendar = getPendingCalendar();
    const title = calendar?.subject?.trim() || generateMeetingTitle();
    return { title, calendar };
  }, [generateMeetingTitle]);

  const startBackendRecording = useCallback(
    async (source: 'home_page' | 'sidebar_auto' | 'sidebar_direct' | 'detected_call'): Promise<{ title: string; sessionId?: string }> => {
      const { title, calendar: pendingCal } = snapshotPendingForStart();
      setMeetingTitle(title);
      setStatus(RecordingStatus.STARTING, 'Initializing recording...');

      let sessionId: string | undefined;
      const mic = selectedDevices?.micDevice || null;
      const system = selectedDevices?.systemDevice || null;
      try {
        if (source === 'detected_call') {
          sessionId = await recordingService.startDetectedCall(mic, system, title);
        } else {
          await recordingService.startRecordingWithDevices(mic, system, title);
        }
      } catch (error) {
        // A delayed retry of a detected start needs fresh meeting evidence.
        if (source !== 'detected_call') await offerSpeakerDetectionCancellation(error, () => startBackendRecording(source));
        throw error;
      }

      beginRecordingCalendar(pendingCal);
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
      snapshotPendingForStart,
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
      const { title } = await startBackendRecording('home_page');
      console.log('Backend recording started successfully');
      console.log('Recording title:', title);
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
            const { title } = await startBackendRecording('sidebar_auto');
            console.log('Auto-start backend recording completed:', title);
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
        const { title } = await startBackendRecording('sidebar_direct');
        console.log('Backend recording completed:', title);
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
