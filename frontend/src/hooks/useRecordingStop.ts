import { useEffect, useRef } from 'react';
import { useRouter } from 'next/navigation';
import { toast } from 'sonner';
import type { RecordingStoppedPayload } from '@/services/recordingService';
import { recordingOutcome, recordingRecoveryMessage } from '@/lib/recording-outcome';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { currentRoute } from '@/lib/current-route';
import { errorText } from '@/lib/errorText';
import Analytics from '@/lib/analytics';
import { clearActiveRecordingCalendar } from '@/lib/meetingCalendar';
import { detectAndCacheSummaryLanguage } from '@/lib/summary-language-preferences';

/** App-wide post-processing for a Stop the native core has already persisted. */
export function useRecordingStop() {
  const { subscribeStopped } = useRecordingState();
  const { transcriptsRef, flushBuffer, clearTranscripts, markMeetingAsSaved } = useTranscripts();
  const { refetchMeetings, setIsMeetingActive } = useSidebar();
  const router = useRouter();

  const finish = async (payload: RecordingStoppedPayload, isCurrent: () => boolean) => {
    if (payload.session_id) clearActiveRecordingCalendar(payload.session_id);
    if (!isCurrent()) return;
    setIsMeetingActive(false);
    const meetingId = payload.meeting_id;
    if (!meetingId) {
      toast.error('Could not add recording to the library', { description: errorText(payload.message) });
      return;
    }

    // Capture the stopped session's UI data before awaiting anything. Native
    // Stop owns the saved transcript, so these lines are only for preferences and analytics.
    flushBuffer();
    const transcripts = [...transcriptsRef.current];
    clearTranscripts();
    void markMeetingAsSaved(payload.recording_id ?? meetingId);
    void refetchMeetings();

    const audioOnly = payload.recording_mode === 'audio_only';
    const outcome = recordingOutcome(payload);
    const recoveryMessage = recordingRecoveryMessage(outcome, payload.audio_saved);
    (recoveryMessage ? toast.warning : toast.success)(recoveryMessage ? 'Meeting saved with warnings' : 'Recording saved', {
      description: recoveryMessage || (audioOnly
        ? 'Audio saved. Open the meeting and choose Transcribe when ready.'
        : `${payload.transcript_count ?? transcripts.length} transcript segments saved.`),
      action: {
        label: 'View meeting',
        onClick: () => router.push(`/meeting-details?id=${meetingId}`),
      },
    });

    try {
      if (!audioOnly) await detectAndCacheSummaryLanguage(meetingId, transcripts.map(line => line.text));
    } catch {
      toast.warning('Could not set the summary language for the saved meeting');
    }

    const route = currentRoute();
    const meetingRoute = `/meeting-details?id=${meetingId}&source=recording`;
    if (isCurrent() && route.path === '/') {
      router.push(meetingRoute);
      Analytics.trackPageView('meeting_details');
    } else if (isCurrent() && route.path === '/meeting-details' && route.id === meetingId) {
      router.replace(meetingRoute);
    }

    try {
      const duration = transcripts.at(-1)?.audio_end_time ?? 0;
      const words = transcripts.reduce((total, line) => total + line.text.split(/\s+/).length, 0);
      await Analytics.trackMeetingCompleted(meetingId, {
        duration_seconds: duration,
        transcript_segments: payload.transcript_count ?? transcripts.length,
        transcript_word_count: words,
        words_per_minute: duration > 0 ? words * 60 / duration : 0,
        meetings_today: await Analytics.getMeetingsCountToday(),
      });
      await Analytics.updateMeetingCount();
    } catch {
      console.warn('Could not update recording analytics.');
    }
  };

  const latest = useRef(finish);
  latest.current = finish;
  useEffect(() => subscribeStopped((payload, isCurrent) => {
    void latest.current(payload, isCurrent).catch(() => {
      console.warn('Could not finish updating the saved meeting view.');
    });
  }), [subscribeStopped]);
}
