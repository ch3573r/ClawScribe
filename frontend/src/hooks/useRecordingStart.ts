import { useState, useEffect, useCallback } from 'react';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { useConfig } from '@/contexts/ConfigContext';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';
import { recordingService } from '@/services/recordingService';
import Analytics from '@/lib/analytics';
import { showRecordingNotification } from '@/lib/recordingNotification';
import { getPendingCalendar, beginRecordingCalendar } from '@/lib/meetingCalendar';

interface UseRecordingStartReturn {
  handleRecordingStart: () => Promise<void>;
  isAutoStarting: boolean;
}

function isTranscriptionReadinessError(error: unknown): boolean {
  const message = (error instanceof Error ? error.message : String(error)).toLowerCase();
  return (
    message.includes('transcription') ||
    message.includes('speech recognition') ||
    message.includes('model') ||
    message.includes('provider')
  );
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
export function useRecordingStart(
  isRecording: boolean,
  setIsRecording: (value: boolean) => void
): UseRecordingStartReturn {
  const [isAutoStarting, setIsAutoStarting] = useState(false);

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
    async (source: 'home_page' | 'sidebar_auto' | 'sidebar_direct') => {
      const { title, calendar: pendingCal } = snapshotPendingForStart();
      setMeetingTitle(title);
      setStatus(RecordingStatus.STARTING, 'Initializing recording...');

      await recordingService.startRecordingWithDevices(
        selectedDevices?.micDevice || null,
        selectedDevices?.systemDevice || null,
        title,
      );

      beginRecordingCalendar(pendingCal);
      setIsRecording(true);
      // TranscriptProvider resets on recording-started before accepting new events.
      setIsMeetingActive(true);
      Analytics.trackButtonClick('start_recording', source);
      void showRecordingNotification();
      return title;
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

  // Handle manual recording start (from button click)
  const handleRecordingStart = useCallback(async () => {
    try {
      console.log('handleRecordingStart called - starting backend recording');
      const title = await startBackendRecording('home_page');
      console.log('Backend recording started successfully');
      console.log('Recording title:', title);
    } catch (error) {
      console.error('Failed to start recording:', error);
      setStatus(RecordingStatus.ERROR, error instanceof Error ? error.message : 'Failed to start recording');
      setIsRecording(false); // Reset state on error
      Analytics.trackButtonClick('start_recording_error', 'home_page');
      if (isTranscriptionReadinessError(error)) return;
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
            const title = await startBackendRecording('sidebar_auto');
            console.log('Auto-start backend recording completed:', title);
          } catch (error) {
            console.error('Failed to auto-start recording:', error);
            setStatus(RecordingStatus.ERROR, error instanceof Error ? error.message : 'Failed to auto-start recording');
            if (!isTranscriptionReadinessError(error)) {
              alert('Failed to start recording. Check console for details.');
            }
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
    selectedDevices,
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
        const title = await startBackendRecording('sidebar_direct');
        console.log('Backend recording completed:', title);
      } catch (error) {
        console.error('Failed to start recording from sidebar:', error);
        setStatus(RecordingStatus.ERROR, error instanceof Error ? error.message : 'Failed to start recording from sidebar');
        if (!isTranscriptionReadinessError(error)) {
          alert('Failed to start recording. Check console for details.');
        }
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
    selectedDevices,
    setStatus,
    startBackendRecording,
  ]);

  return {
    handleRecordingStart,
    isAutoStarting,
  };
}
