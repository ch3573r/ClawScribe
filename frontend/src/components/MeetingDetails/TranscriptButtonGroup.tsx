"use client";

import { useState, useCallback, useEffect, useRef } from 'react';
import { Button } from '@/components/ui/button';
import { ButtonGroup } from '@/components/ui/button-group';
import { Copy, FolderOpen, RefreshCw, Users } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import Analytics from '@/lib/analytics';
import { RetranscribeDialog } from './RetranscribeDialog';

interface SpeakerDiarizationProgress {
  meeting_id: string;
  stage: string;
  progress_percentage: number;
  message: string;
}

interface SpeakerDiarizationComplete {
  meeting_id: string;
  speaker_count: number;
  updated_segments: number;
}

interface SpeakerDiarizationError {
  meeting_id: string;
  error: string;
}

interface TranscriptButtonGroupProps {
  transcriptCount: number;
  onCopyTranscript: () => void;
  onOpenMeetingFolder: () => Promise<void>;
  meetingId?: string;
  meetingFolderPath?: string | null;
  showSpeakerAttribution?: boolean;
  onRefetchTranscripts?: () => Promise<void>;
}


export function TranscriptButtonGroup({
  transcriptCount,
  onCopyTranscript,
  onOpenMeetingFolder,
  meetingId,
  meetingFolderPath,
  showSpeakerAttribution = true,
  onRefetchTranscripts,
}: TranscriptButtonGroupProps) {
  const [showRetranscribeDialog, setShowRetranscribeDialog] = useState(false);
  const [isDiarizing, setIsDiarizing] = useState(false);
  const [diarizationMessage, setDiarizationMessage] = useState<string | null>(null);
  const diarizationToastIdRef = useRef<string | number | null>(null);

  const showDiarizationProgress = useCallback((message: string, progress?: number) => {
    const id = diarizationToastIdRef.current ?? `speaker-diarization-${meetingId ?? 'current'}`;
    diarizationToastIdRef.current = id;
    toast.loading('Detecting speakers', {
      id,
      description: typeof progress === 'number' ? `${progress}% - ${message}` : message,
      duration: Infinity,
    });
  }, [meetingId]);

  const clearDiarizationProgress = useCallback(() => {
    if (diarizationToastIdRef.current !== null) {
      toast.dismiss(diarizationToastIdRef.current);
      diarizationToastIdRef.current = null;
    }
  }, []);

  const handleRetranscribeComplete = useCallback(async () => {
    // Refetch transcripts to show the updated data
    if (onRefetchTranscripts) {
      await onRefetchTranscripts();
    }
  }, [onRefetchTranscripts]);

  useEffect(() => {
    if (!meetingId) return;

    const unlistenCallbacks: Array<() => void> = [];

    void listen<SpeakerDiarizationProgress>('speaker-diarization-progress', (event) => {
      if (event.payload.meeting_id !== meetingId) return;
      const running = event.payload.stage !== 'complete';
      setIsDiarizing(running);
      setDiarizationMessage(event.payload.message);
      if (running) {
        showDiarizationProgress(event.payload.message, event.payload.progress_percentage);
      }
    }).then((unlisten) => unlistenCallbacks.push(unlisten));

    void listen<SpeakerDiarizationComplete>('speaker-diarization-complete', async (event) => {
      if (event.payload.meeting_id !== meetingId) return;
      setIsDiarizing(false);
      setDiarizationMessage(null);
      clearDiarizationProgress();
      toast.success('Speaker labels applied', {
        description: `${event.payload.updated_segments} transcript segments updated across ${event.payload.speaker_count} speaker${event.payload.speaker_count === 1 ? '' : 's'}.`,
      });
      await onRefetchTranscripts?.();
    }).then((unlisten) => unlistenCallbacks.push(unlisten));

    void listen<SpeakerDiarizationError>('speaker-diarization-error', (event) => {
      if (event.payload.meeting_id !== meetingId) return;
      setIsDiarizing(false);
      setDiarizationMessage(null);
      clearDiarizationProgress();
      toast.error('Speaker diarization failed', {
        description: event.payload.error,
      });
    }).then((unlisten) => unlistenCallbacks.push(unlisten));

    return () => {
      unlistenCallbacks.forEach((unlisten) => unlisten());
    };
  }, [clearDiarizationProgress, meetingId, onRefetchTranscripts, showDiarizationProgress]);

  const handleRunSpeakerDiarization = useCallback(async () => {
    if (!meetingId || !meetingFolderPath) return;
    setIsDiarizing(true);
    setDiarizationMessage('Starting speaker diarization...');
    showDiarizationProgress('Starting speaker diarization...', 0);
    try {
      Analytics.trackButtonClick('speaker_diarization', 'meeting_details');
      await invoke('start_speaker_diarization_command', {
        meetingId,
        meetingFolderPath,
        segmentationModelPath: null,
        embeddingModelPath: null,
        numSpeakers: null,
        preserveExistingLabels: false,
      });
    } catch (error) {
      setIsDiarizing(false);
      setDiarizationMessage(null);
      clearDiarizationProgress();
      toast.error('Could not start speaker diarization', {
        description: String(error),
      });
    }
  }, [clearDiarizationProgress, meetingFolderPath, meetingId, showDiarizationProgress]);

  return (
    <div className="flex shrink-0 items-center justify-end gap-2">
      <ButtonGroup>
        <Button
          variant="outline"
          size="sm"
          onClick={() => {
            Analytics.trackButtonClick('copy_transcript', 'meeting_details');
            onCopyTranscript();
          }}
          disabled={transcriptCount === 0}
          title={transcriptCount === 0 ? 'No transcript available' : 'Copy Transcript'}
        >
          <Copy />
          <span className="hidden 2xl:inline">Copy</span>
        </Button>

        <Button
          size="sm"
          variant="outline"
          className="2xl:px-4"
          onClick={() => {
            Analytics.trackButtonClick('open_recording_folder', 'meeting_details');
            onOpenMeetingFolder();
          }}
          title="Open Recording Folder"
        >
          <FolderOpen className="2xl:mr-2" size={18} />
          <span className="hidden 2xl:inline">Recording</span>
        </Button>

        {showSpeakerAttribution && meetingId && meetingFolderPath && (
          <Button
            size="sm"
            variant="outline"
            className="2xl:px-4"
            onClick={() => void handleRunSpeakerDiarization()}
            disabled={transcriptCount === 0 || isDiarizing}
            title={diarizationMessage ?? "Detect speakers from the saved recording"}
          >
            {isDiarizing ? (
              <RefreshCw className="animate-spin 2xl:mr-2" size={18} />
            ) : (
              <Users className="2xl:mr-2" size={18} />
            )}
            <span className="hidden 2xl:inline">Speakers</span>
          </Button>
        )}

        {meetingId && meetingFolderPath && (
          <Button
            size="sm"
            variant="outline"
            className="bg-primary/10 hover:bg-primary/20 border-primary/30 text-foreground 2xl:px-4"
            onClick={() => {
              Analytics.trackButtonClick('enhance_transcript', 'meeting_details');
              setShowRetranscribeDialog(true);
            }}
            title="Retranscribe to enhance your recorded audio"
          >
            <RefreshCw className="2xl:mr-2" size={18} />
            <span className="hidden 2xl:inline">Enhance</span>
          </Button>
        )}
      </ButtonGroup>

      {meetingId && meetingFolderPath && (
        <RetranscribeDialog
          open={showRetranscribeDialog}
          onOpenChange={setShowRetranscribeDialog}
          meetingId={meetingId}
          meetingFolderPath={meetingFolderPath}
          onComplete={handleRetranscribeComplete}
        />
      )}
    </div>
  );
}
