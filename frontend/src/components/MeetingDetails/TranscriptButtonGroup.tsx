"use client";

import { useState, useCallback, useEffect } from 'react';
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
  onRefetchTranscripts?: () => Promise<void>;
}


export function TranscriptButtonGroup({
  transcriptCount,
  onCopyTranscript,
  onOpenMeetingFolder,
  meetingId,
  meetingFolderPath,
  onRefetchTranscripts,
}: TranscriptButtonGroupProps) {
  const [showRetranscribeDialog, setShowRetranscribeDialog] = useState(false);
  const [isDiarizing, setIsDiarizing] = useState(false);
  const [diarizationMessage, setDiarizationMessage] = useState<string | null>(null);

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
      setIsDiarizing(event.payload.stage !== 'complete');
      setDiarizationMessage(event.payload.message);
    }).then((unlisten) => unlistenCallbacks.push(unlisten));

    void listen<SpeakerDiarizationComplete>('speaker-diarization-complete', async (event) => {
      if (event.payload.meeting_id !== meetingId) return;
      setIsDiarizing(false);
      setDiarizationMessage(null);
      toast.success('Speaker labels applied', {
        description: `${event.payload.updated_segments} transcript segments updated across ${event.payload.speaker_count} speaker${event.payload.speaker_count === 1 ? '' : 's'}.`,
      });
      await onRefetchTranscripts?.();
    }).then((unlisten) => unlistenCallbacks.push(unlisten));

    void listen<SpeakerDiarizationError>('speaker-diarization-error', (event) => {
      if (event.payload.meeting_id !== meetingId) return;
      setIsDiarizing(false);
      setDiarizationMessage(null);
      toast.error('Speaker diarization failed', {
        description: event.payload.error,
      });
    }).then((unlisten) => unlistenCallbacks.push(unlisten));

    return () => {
      unlistenCallbacks.forEach((unlisten) => unlisten());
    };
  }, [meetingId, onRefetchTranscripts]);

  const handleRunSpeakerDiarization = useCallback(async () => {
    if (!meetingId || !meetingFolderPath) return;
    setIsDiarizing(true);
    setDiarizationMessage('Starting speaker diarization...');
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
      toast.error('Could not start speaker diarization', {
        description: String(error),
      });
    }
  }, [meetingFolderPath, meetingId]);

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
          <span className="hidden lg:inline">Copy</span>
        </Button>

        <Button
          size="sm"
          variant="outline"
          className="xl:px-4"
          onClick={() => {
            Analytics.trackButtonClick('open_recording_folder', 'meeting_details');
            onOpenMeetingFolder();
          }}
          title="Open Recording Folder"
        >
          <FolderOpen className="xl:mr-2" size={18} />
          <span className="hidden lg:inline">Recording</span>
        </Button>

        {meetingId && meetingFolderPath && (
          <Button
            size="sm"
            variant="outline"
            className="xl:px-4"
            onClick={() => void handleRunSpeakerDiarization()}
            disabled={transcriptCount === 0 || isDiarizing}
            title={diarizationMessage ?? "Detect speakers from the saved recording"}
          >
            {isDiarizing ? (
              <RefreshCw className="animate-spin xl:mr-2" size={18} />
            ) : (
              <Users className="xl:mr-2" size={18} />
            )}
            <span className="hidden lg:inline">Speakers</span>
          </Button>
        )}

        {meetingId && meetingFolderPath && (
          <Button
            size="sm"
            variant="outline"
            className="bg-primary/10 hover:bg-primary/20 border-primary/30 text-foreground xl:px-4"
            onClick={() => {
              Analytics.trackButtonClick('enhance_transcript', 'meeting_details');
              setShowRetranscribeDialog(true);
            }}
            title="Retranscribe to enhance your recorded audio"
          >
            <RefreshCw className="xl:mr-2" size={18} />
            <span className="hidden lg:inline">Enhance</span>
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
