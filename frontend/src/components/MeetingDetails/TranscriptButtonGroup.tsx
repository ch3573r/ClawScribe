"use client";

import { useState, useCallback, useEffect, useRef } from 'react';
import { Button } from '@/components/ui/button';
import { ButtonGroup } from '@/components/ui/button-group';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import {
  CheckCircle2,
  ChevronDown,
  Clock,
  Copy,
  Cpu,
  FolderOpen,
  Gauge,
  Hash,
  RefreshCw,
  Route,
  Users,
  Zap,
} from 'lucide-react';
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
  duration_seconds: number;
  processing_seconds: number;
  provider: string;
  embedding_model: string;
  turn_count: number;
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

function formatDuration(seconds: number): string {
  const safeSeconds = Number.isFinite(seconds) && seconds > 0 ? seconds : 0;
  const hours = Math.floor(safeSeconds / 3600);
  const minutes = Math.floor((safeSeconds % 3600) / 60);
  const secs = Math.floor(safeSeconds % 60);

  if (hours > 0) {
    return `${hours}:${minutes.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
  }
  return `${minutes}:${secs.toString().padStart(2, '0')}`;
}

function formatProvider(provider: string): string {
  const normalized = provider.trim().toLowerCase();
  if (normalized === 'directml') return 'DirectML';
  if (normalized === 'cpu') return 'CPU';
  if (!normalized) return 'Unknown provider';
  return provider;
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
  const [diarizationResult, setDiarizationResult] = useState<SpeakerDiarizationComplete | null>(null);
  const [showDiarizationResult, setShowDiarizationResult] = useState(false);
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
      setDiarizationResult(event.payload);
      setShowDiarizationResult(true);
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
      setDiarizationResult(null);
      setShowDiarizationResult(false);
      clearDiarizationProgress();
      toast.error('Speaker diarization failed', {
        description: event.payload.error,
      });
    }).then((unlisten) => unlistenCallbacks.push(unlisten));

    return () => {
      unlistenCallbacks.forEach((unlisten) => unlisten());
    };
  }, [clearDiarizationProgress, meetingId, onRefetchTranscripts, showDiarizationProgress]);

  const handleRunSpeakerDiarization = useCallback(async (numSpeakers: number | null = null) => {
    if (!meetingId || !meetingFolderPath) return;
    const speakerMode = numSpeakers ? `${numSpeakers} speakers` : 'Auto speaker detection';
    setIsDiarizing(true);
    setDiarizationMessage(`Starting ${speakerMode.toLowerCase()}...`);
    setDiarizationResult(null);
    setShowDiarizationResult(false);
    showDiarizationProgress(`Starting ${speakerMode.toLowerCase()}...`, 0);
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
      clearDiarizationProgress();
      toast.error('Could not start speaker diarization', {
        description: String(error),
      });
    }
  }, [clearDiarizationProgress, meetingFolderPath, meetingId, showDiarizationProgress]);

  const speakerDetectionDisabled = transcriptCount === 0 || isDiarizing;
  const diarizationSpeed = diarizationResult && diarizationResult.processing_seconds > 0
    ? diarizationResult.duration_seconds / diarizationResult.processing_seconds
    : null;

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

      <Dialog open={showDiarizationResult} onOpenChange={setShowDiarizationResult}>
        <DialogContent className="sm:max-w-[540px]">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <CheckCircle2 className="h-5 w-5 text-emerald-600" />
              Speaker Diarization Complete
            </DialogTitle>
            <DialogDescription>
              Speaker labels were applied. Benchmark below.
            </DialogDescription>
          </DialogHeader>

          {diarizationResult && (
            <div className="space-y-4 py-2">
              <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
                <div className="rounded-md border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Clock className="h-3.5 w-3.5" />
                    Audio
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">
                    {formatDuration(diarizationResult.duration_seconds)}
                  </p>
                </div>
                <div className="rounded-md border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Gauge className="h-3.5 w-3.5" />
                    Processing
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">
                    {diarizationResult.processing_seconds.toFixed(1)}s
                  </p>
                </div>
                <div className="rounded-md border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Zap className="h-3.5 w-3.5" />
                    Speed
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">
                    {diarizationSpeed ? `${diarizationSpeed.toFixed(1)}x` : '-'}
                  </p>
                </div>
                <div className="rounded-md border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Users className="h-3.5 w-3.5" />
                    Speakers
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">
                    {diarizationResult.speaker_count}
                  </p>
                </div>
                <div className="rounded-md border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Route className="h-3.5 w-3.5" />
                    Turns
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">
                    {diarizationResult.turn_count}
                  </p>
                </div>
                <div className="rounded-md border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Hash className="h-3.5 w-3.5" />
                    Rows
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">
                    {diarizationResult.updated_segments}
                  </p>
                </div>
              </div>

              <div className="space-y-1 text-xs text-muted-foreground">
                <div className="flex items-start gap-1.5">
                  <Cpu className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                  <span>Provider: {formatProvider(diarizationResult.provider)}</span>
                </div>
                <div className="flex items-start gap-1.5">
                  <Users className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                  <span className="break-words">Embedding: {diarizationResult.embedding_model}</span>
                </div>
              </div>
            </div>
          )}

          <DialogFooter>
            <Button variant="outline" onClick={() => setShowDiarizationResult(false)}>
              Close
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
