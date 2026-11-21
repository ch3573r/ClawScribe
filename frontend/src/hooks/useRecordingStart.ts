import { useCallback, useEffect } from 'react';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { useConfig } from '@/contexts/ConfigContext';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';
import { recordingService } from '@/services/recordingService';
import Analytics from '@/lib/analytics';
import { showRecordingNotification } from '@/lib/recordingNotification';
import { offerSpeakerDetectionCancellation } from '@/lib/speakerDetectionCancellation';
import {
  consumeNextCalendarChoice, getActiveRecordingCalendar, getNextCalendarChoice, isReviewedCalendar,
  microsoftCalendarRevision, setActiveRecordingCalendar,
} from '@/lib/meetingCalendar';
import { readRecordingCalendar, resolveRecordingCalendar } from '@/services/recordingCalendarService';
import { toast } from 'sonner';
import { readPinnedSummaryLanguageDefault } from '@/lib/summary-language-preferences';
import {
  isDiskSpaceRefusal, LIVE_MODEL_FAILURE_MESSAGE, liveModelStartFailure, startErrorText, transcriptionSettingsPath,
} from '@/lib/start-error';
import { openStorageDialog } from '@/lib/storage-dialog-controller';

type StartSource = 'home_page' | 'sidebar_auto' | 'sidebar_direct' | 'detected_call' | 'remote_request';
let startInProgress = false;

export const DEVICE_PREFERENCES_UNAVAILABLE = 'recording_device_preferences_unavailable';
export const START_REQUEST_EXPIRED = 'recording_request_expired';
export const START_BUSY = 'recording_start_busy';

export function reportStartFailure(error: unknown): void {
  const message = startErrorText(error);
  const modelFailure = liveModelStartFailure(error);
  if (message === START_REQUEST_EXPIRED) return;
  if (message === START_BUSY) {
    toast.info('Recording is busy. During a save, choose Start after saving to record again.');
    return;
  }
  if (isDiskSpaceRefusal(error)) {
    toast.error('Not enough disk space', {
      description: message,
      duration: 15000,
      action: { label: 'Review storage', onClick: openStorageDialog },
    });
  } else if (modelFailure) {
    toast.error('Live transcription unavailable', {
      description: LIVE_MODEL_FAILURE_MESSAGE[modelFailure],
      action: {
        label: 'Transcription settings',
        onClick: () => window.location.assign(transcriptionSettingsPath()),
      },
    });
  } else if (message === DEVICE_PREFERENCES_UNAVAILABLE) {
    toast.error('Could not verify audio device preferences', {
      action: { label: 'Recording settings', onClick: () => window.location.assign('/settings?tab=recording') },
    });
  } else {
    toast.error('Could not start recording. Check your audio devices and microphone permissions, then try again.');
  }
}

function meetingTitleAt(timestamp: number): string {
  const now = new Date(timestamp);
  const [day, month, hour, minute, second] = [
    now.getDate(), now.getMonth() + 1, now.getHours(), now.getMinutes(), now.getSeconds(),
  ].map(value => String(value).padStart(2, '0'));
  return `Meeting ${day}_${month}_${String(now.getFullYear()).slice(-2)}_${hour}_${minute}_${second}`;
}

export function useStartRecordingSession() {
  const { setIsMeetingActive } = useSidebar();
  const { selectedDevices, devicePreferencesStatus } = useConfig();
  const { setStatus, isRecording, isStopping, isProcessing, isSaving } = useRecordingState();

  const start = useCallback(async (source: StartSource, requestId?: string): Promise<string> => {
    if (startInProgress || isRecording || isStopping || isProcessing || isSaving) throw START_BUSY;
    if (devicePreferencesStatus !== 'ready') throw DEVICE_PREFERENCES_UNAVAILABLE;
    startInProgress = true;
    const now = Date.now();
    const choice = getNextCalendarChoice();
    const microsoftRevision = microsoftCalendarRevision;
    setStatus(RecordingStatus.STARTING, 'Initializing recording…');
    try {
      const sessionId = await recordingService.startRecordingWithDevices(
        selectedDevices.micDevice, selectedDevices.systemDevice, meetingTitleAt(now), choice.link, requestId,
        readPinnedSummaryLanguageDefault(),
      );
      consumeNextCalendarChoice(choice);
      setIsMeetingActive(true);
      Analytics.trackButtonClick('start_recording', source);
      void showRecordingNotification();
      // Calendar I/O begins only once native capture is running. The write is
      // scoped to that session; stopping or a correction can win the race.
      void readRecordingCalendar(sessionId).then(draft => {
        if (microsoftRevision !== microsoftCalendarRevision && draft.link?.source === 'microsoft' && !isReviewedCalendar(draft.link)) return;
        const previous = getActiveRecordingCalendar(sessionId);
        if (previous && previous.revision > draft.revision) return;
        const initial = previous?.revision === draft.revision
          ? { ...previous, startedAt: now } : { sessionId, startedAt: now, ...draft, status: 'none' as const, matches: [] };
        if (setActiveRecordingCalendar(initial) && !choice.link && !choice.skip && draft.revision === 0) {
          return resolveRecordingCalendar(initial, now, microsoftRevision);
        }
      }).catch(() => {});
      return sessionId;
    } catch (error) {
      setStatus(RecordingStatus.ERROR, 'Could not start the recording.');
      Analytics.trackButtonClick('start_recording_error', source);
      // A delayed retry requires a new native request and fresh call evidence.
      if (!requestId) await offerSpeakerDetectionCancellation(error, () => start(source)).catch(() => {});
      throw error;
    } finally {
      startInProgress = false;
    }
  }, [devicePreferencesStatus, selectedDevices, setIsMeetingActive, setStatus, isRecording, isStopping, isProcessing, isSaving]);
  return start;
}

export function useRecordingStart() {
  const start = useStartRecordingSession();

  const handleRecordingStart = useCallback(async () => {
    try {
      await start('home_page');
    } catch (error) {
      if (error === START_BUSY) reportStartFailure(error);
      if (!liveModelStartFailure(error) && error !== START_BUSY) throw error;
    }
  }, [start]);

  useEffect(() => {
    if (sessionStorage.getItem('autoStartRecording') !== 'true') return;
    sessionStorage.removeItem('autoStartRecording');
    void start('sidebar_auto').catch(reportStartFailure);
  }, [start]);

  useEffect(() => {
    const onStart = () => { void start('sidebar_direct').catch(reportStartFailure); };
    window.addEventListener('start-recording-from-sidebar', onStart);
    return () => window.removeEventListener('start-recording-from-sidebar', onStart);
  }, [start]);

  return { handleRecordingStart };
}
