import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import type { ModelConfig } from '@/components/ModelSettingsModal';
import { recordingBlocksAutoSummary, type RecordingOutcome } from '@/lib/recording-outcome';
import { fetchAllMeetingTranscripts } from '@/lib/meetingTranscripts';
import { formatTranscriptLine } from '@/lib/transcriptFormatting';
import { getMeetingContext } from '@/lib/meetingContext';
import { getSourceAttribution } from '@/lib/sourceAttribution';
import { shouldShowSpeakerAttribution } from '@/lib/speaker-attribution';
import {
  detectAndCacheSummaryLanguage,
  readCachedDetectedSummaryLanguage,
  readMeetingSummaryLanguage,
} from '@/lib/summary-language-preferences';

/**
 * Starts the automatic summary for a meeting that was just saved, without the
 * meeting page being open. The meeting page does the same when it is opened
 * with `source=recording`; this is used when the user stays on another page.
 *
 * It follows the same readiness rules as the page: a recording that lost audio
 * or transcript never starts a summary, and nothing starts unless a summary
 * model is already configured. Returns true when generation was requested.
 */
export interface AutoSummaryRequest {
  meetingId: string;
  folderPath?: string | null;
  modelConfig: ModelConfig;
  startSummaryPolling: (
    meetingId: string,
    processId: string,
    onUpdate: (result: any) => void | Promise<void>,
    options?: { background?: boolean },
  ) => void;
  refetchMeetings: () => Promise<void>;
  openMeeting: (meetingId: string) => void;
}

async function summaryLanguageFor(meetingId: string, texts: string[]): Promise<string | null> {
  try {
    const saved = await readMeetingSummaryLanguage(meetingId);
    if (saved.language) return saved.language;
  } catch {
    // Fall back to detection below.
  }
  try {
    const cached = await readCachedDetectedSummaryLanguage(meetingId);
    if (cached) return cached;
  } catch {
    // Fall back to detection below.
  }
  try {
    return (await detectAndCacheSummaryLanguage(meetingId, texts)).language;
  } catch {
    return null;
  }
}

export async function startAutomaticSummary(request: AutoSummaryRequest): Promise<boolean> {
  const { meetingId, modelConfig } = request;
  try {
    const outcome = await invoke<RecordingOutcome | null>('get_recording_outcome', { meetingId });
    if (recordingBlocksAutoSummary(outcome)) return false;
    const config = await invoke<{ model?: string }>('api_get_model_config');
    if (!config?.model) return false;

    const transcripts = await fetchAllMeetingTranscripts(invoke, meetingId);
    if (transcripts.length === 0) return false;

    const includeSpeaker = shouldShowSpeakerAttribution(
      getSourceAttribution(), Boolean(request.folderPath), transcripts,
    );
    const customPrompt = await getMeetingContext(meetingId);
    const summaryLanguage = await summaryLanguageFor(meetingId, transcripts.map(t => t.text));

    toast.info('Generating summary…', {
      description: `Using ${modelConfig.provider}/${modelConfig.model}`,
      duration: 3000,
    });
    const result = await invoke<{ process_id: string }>('api_process_transcript', {
      text: transcripts.map(t => formatTranscriptLine(t, { includeSpeaker })).join('\n'),
      model: modelConfig.provider,
      modelName: modelConfig.model,
      meetingId,
      chunkSize: 40000,
      overlap: 1000,
      customPrompt,
      templateId: null,
      summaryLanguage,
    });

    request.startSummaryPolling(meetingId, result.process_id, async update => {
      if (update.status === 'completed') {
        toast.success('Summary ready', {
          id: `summary-ready:${meetingId}`,
          action: { label: 'View meeting', onClick: () => request.openMeeting(meetingId) },
          duration: 8000,
        });
        await request.refetchMeetings().catch(() => {});
      } else if (update.status === 'error' || update.status === 'failed') {
        toast.error('Could not generate the summary', {
          description: update.error || 'Open the meeting and try again.',
          action: { label: 'View meeting', onClick: () => request.openMeeting(meetingId) },
        });
      }
    }, { background: true });
    return true;
  } catch (error) {
    // A failed automatic start must never block the saved meeting.
    toast.error('Could not start the automatic summary', {
      description: error instanceof Error ? error.message : String(error),
      action: { label: 'View meeting', onClick: () => request.openMeeting(meetingId) },
    });
    return false;
  }
}
