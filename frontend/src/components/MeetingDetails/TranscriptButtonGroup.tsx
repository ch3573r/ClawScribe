"use client";

import { useState, useCallback, useEffect } from 'react';
import { Button } from '@/components/ui/button';
import { ButtonGroup } from '@/components/ui/button-group';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import {
  ChevronDown,
  Copy,
  FolderOpen,
  RefreshCw,
  Users,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import Analytics from '@/lib/analytics';
import { RetranscribeDialog } from './RetranscribeDialog';
import {
  SpeakerDiarizationDialog,
  SpeakerDiarizationComplete,
  SpeakerDiarizationProgress,
} from './SpeakerDiarizationDialog';

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
  const [showDiarizationDialog, setShowDiarizationDialog] = useState(false);
  const [isDiarizing, setIsDiarizing] = useState(false);
  const [diarizationMessage, setDiarizationMessage] = useState<string | null>(null);
  const [diarizationProgress, setDiarizationProgress] = useState<SpeakerDiarizationProgress | null>(null);
  const [diarizationResult, setDiarizationResult] = useState<SpeakerDiarizationComplete | null>(null);
  const [diarizationError, setDiarizationError] = useState<string | null>(null);
  const [diarizationMode, setDiarizationMode] = useState<string | null>(null);

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
      setDiarizationProgress(event.payload);
      setShowDiarizationDialog(true);
    }).then((unlisten) => unlistenCallbacks.push(unlisten));

    void listen<SpeakerDiarizationComplete>('speaker-diarization-complete', async (event) => {
      if (event.payload.meeting_id !== meetingId) return;
      setIsDiarizing(false);
      setDiarizationMessage(null);
      setDiarizationProgress(null);
      setDiarizationResult(event.payload);
      setDiarizationError(null);
      setShowDiarizationDialog(true);
      toast.success('Speaker labels applied', {
        description: `${event.payload.updated_segments} transcript segments updated across ${event.payload.speaker_count} speaker${event.payload.speaker_count === 1 ? '' : 's'}.`,
      });
      await onRefetchTranscripts?.();
    }).then((unlisten) => unlistenCallbacks.push(unlisten));

    void listen<SpeakerDiarizationError>('speaker-diarization-error', async (event) => {
      if (event.payload.meeting_id !== meetingId) return;
      setIsDiarizing(false);
      setDiarizationMessage(null);
      setDiarizationProgress(null);
      setDiarizationResult(null);
      setDiarizationError(event.payload.error);
      setShowDiarizationDialog(true);
      toast.error('Speaker diarization failed', {
        description: event.payload.error,
      });
      await onRefetchTranscripts?.();
    }).then((unlisten) => unlistenCallbacks.push(unlisten));

    return () => {
      unlistenCallbacks.forEach((unlisten) => unlisten());
    };
  }, [meetingId, onRefetchTranscripts]);

  const handleRunSpeakerDiarization = useCallback(async (numSpeakers: number | null = null) => {
    if (!meetingId || !meetingFolderPath) return;
    const speakerMode = numSpeakers ? `${numSpeakers} speakers` : 'Auto speaker detection';
    setIsDiarizing(true);
    setDiarizationMessage(`Starting ${speakerMode.toLowerCase()}...`);
    setDiarizationMode(speakerMode);
    setDiarizationProgress({
      meeting_id: meetingId,
      stage: 'starting',
      progress_percentage: 0,
      message: `Starting ${speakerMode.toLowerCase()}...`,
    });
    setDiarizationResult(null);
    setDiarizationError(null);
    setShowDiarizationDialog(true);
    try {
      Analytics.trackButtonClick(numSpeakers ? `speaker_diarization_${numSpeakers}` : 'speaker_diarization_auto', 'meeting_details');
      await invoke('start_speaker_diarization_command', {
        meetingId,
        meetingFolderPath,
        segmentationModelPath: null,
        embeddingModelPath: null,
        embeddingModelId: null,
        numSpeakers,
        preserveExistingLabels: false,
      });
    } catch (error) {
      setIsDiarizing(false);
      setDiarizationMessage(null);
      setDiarizationProgress(null);
      setDiarizationError(String(error));
      setShowDiarizationDialog(true);
      toast.error('Could not start speaker diarization', {
        description: String(error),
      });
    }
  }, [meetingFolderPath, meetingId]);

  const speakerDetectionDisabled = transcriptCount === 0 || isDiarizing;

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
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                size="sm"
                variant="outline"
                className="2xl:px-4"
                disabled={speakerDetectionDisabled}
                title={diarizationMessage ?? "Detect speakers from the saved recording"}
              >
                {isDiarizing ? (
                  <RefreshCw className="animate-spin 2xl:mr-2" size={18} />
                ) : (
                  <Users className="2xl:mr-2" size={18} />
                )}
                <span className="hidden 2xl:inline">Speakers</span>
                <ChevronDown className="ml-1 size-3.5 opacity-70" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-48">
              <DropdownMenuLabel>Speaker count</DropdownMenuLabel>
              <DropdownMenuItem onSelect={() => void handleRunSpeakerDiarization(null)}>
                Auto detect
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              {[2, 3, 4, 5, 6].map((count) => (
                <DropdownMenuItem key={count} onSelect={() => void handleRunSpeakerDiarization(count)}>
                  {count} speakers
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
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

      <SpeakerDiarizationDialog
        open={showDiarizationDialog}
        onOpenChange={setShowDiarizationDialog}
        isProcessing={isDiarizing}
        progress={diarizationProgress}
        result={diarizationResult}
        error={diarizationError}
        speakerMode={diarizationMode}
        onClearError={() => {
          setDiarizationError(null);
          setDiarizationProgress(null);
          setDiarizationResult(null);
          setShowDiarizationDialog(false);
        }}
      />
    </div>
  );
}
