"use client";

import { useState, useCallback, useEffect, useRef } from 'react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { ActionIcon } from './ActionIcon';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import {
  MoreHorizontal,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import Analytics from '@/lib/analytics';
import { canDetectSpeakers } from '@/lib/speaker-attribution';
import { DEFAULT_PLAYER_BINDINGS, playerKeycaps, type PlayerBindings } from '@/lib/player-bindings';
import { RetranscribeDialog } from './RetranscribeDialog';
import {
  SpeakerDiarizationDialog,
  SpeakerDiarizationComplete,
  SpeakerDiarizationProgress,
} from './SpeakerDiarizationDialog';
import { autoAppliedSpeakerMessage } from '@/lib/speaker-labels';

interface SpeakerDiarizationError {
  meeting_id: string;
  error: string;
}

interface TranscriptButtonGroupProps {
  bindings?: PlayerBindings;
  onFindReplace?: () => void; onAddBookmark?: () => void;
  correctionActions?: { busy: boolean; canUndo: boolean; canRestore: boolean; undo: () => void; restore: () => void };
  transcriptCount: number;
  hasSpeakerLabels?: boolean;
  speakerLabelsComplete?: boolean;
  transcribeOpen: boolean;
  onTranscribeOpenChange: (open: boolean) => void;
  onCopyTranscript: () => void;
  onFind?: () => void;
  meetingId?: string;
  meetingFolderPath?: string | null;
  audioAvailable?: boolean;
  onRefetchTranscripts?: () => Promise<void>;
}

export function TranscriptButtonGroup({
  bindings = DEFAULT_PLAYER_BINDINGS,
  onFindReplace, onAddBookmark, correctionActions,
  transcriptCount,
  hasSpeakerLabels = false,
  speakerLabelsComplete = true,
  transcribeOpen,
  onTranscribeOpenChange,
  onCopyTranscript,
  onFind,
  meetingId,
  meetingFolderPath,
  audioAvailable = true,
  onRefetchTranscripts,
}: TranscriptButtonGroupProps) {
  const [detectionRequest, setDetectionRequest] = useState<{ numSpeakers: number | null } | null>(null);
  const menuTrigger = useRef<HTMLButtonElement>(null);
  const startingDetection = useRef(false);
  const [showDiarizationDialog, setShowDiarizationDialog] = useState(false);
  const [isDiarizing, setIsDiarizing] = useState(false);
  const [isCancelling, setIsCancelling] = useState(false);
  const [diarizationMessage, setDiarizationMessage] = useState<string | null>(null);
  const [diarizationProgress, setDiarizationProgress] = useState<SpeakerDiarizationProgress | null>(null);
  const [diarizationResult, setDiarizationResult] = useState<SpeakerDiarizationComplete | null>(null);
  const [diarizationError, setDiarizationError] = useState<string | null>(null);
  const [diarizationMode, setDiarizationMode] = useState<string | null>(null);
  const meetingIdRef = useRef(meetingId);
  const onRefetchTranscriptsRef = useRef(onRefetchTranscripts);
  const lastCompletionKeyRef = useRef<string | null>(null);
  const handledErrorRef = useRef(false);

  useEffect(() => {
    meetingIdRef.current = meetingId;
  }, [meetingId]);

  useEffect(() => {
    onRefetchTranscriptsRef.current = onRefetchTranscripts;
  }, [onRefetchTranscripts]);

  const handleRetranscribeComplete = useCallback(async () => {
    // Refetch transcripts to show the updated data
    if (onRefetchTranscripts) {
      await onRefetchTranscripts();
    }
  }, [onRefetchTranscripts]);

  const completionKey = useCallback((payload: SpeakerDiarizationComplete) => {
    return [
      payload.meeting_id,
      payload.speaker_count,
      payload.updated_segments,
      payload.duration_seconds,
      payload.processing_seconds,
      payload.provider,
      payload.embedding_model,
      payload.turn_count,
    ].join(':');
  }, []);

  const showDiarizationComplete = useCallback((payload: SpeakerDiarizationComplete) => {
    const key = completionKey(payload);
    if (lastCompletionKeyRef.current === key) {
      return;
    }
    lastCompletionKeyRef.current = key;
    handledErrorRef.current = false;
    setIsDiarizing(false);
    setIsCancelling(false);
    setDiarizationMessage(null);
    setDiarizationProgress(null);
    setDiarizationResult(payload);
    setDiarizationError(null);
    setShowDiarizationDialog(true);
    toast.success('Speaker labels applied', {
      description: autoAppliedSpeakerMessage(payload.auto_applied_names)
        ?? `${payload.updated_segments} transcript segments updated across ${payload.speaker_count} speaker${payload.speaker_count === 1 ? '' : 's'}.`,
    });
    void onRefetchTranscriptsRef.current?.();
  }, [completionKey]);

  const showDiarizationError = useCallback((message: string) => {
    if (handledErrorRef.current) {
      return;
    }
    handledErrorRef.current = true;
    setIsDiarizing(false);
    setIsCancelling(false);
    setDiarizationMessage(null);
    setDiarizationProgress(null);
    setDiarizationResult(null);
    if (message === 'Speaker detection cancelled') {
      setDiarizationError(null);
      setShowDiarizationDialog(false);
      toast.info('Speaker detection cancelled');
      return;
    }
    setDiarizationError(message);
    setShowDiarizationDialog(true);
    toast.error('Speaker diarization failed', {
      description: message,
    });
    void onRefetchTranscriptsRef.current?.();
  }, []);

  useEffect(() => {
    let cleanedUp = false;
    let observedEvent = false;
    const unlistenCallbacks: Array<() => void> = [];

    const trackUnlistener = (unlisten: () => void) => {
      if (cleanedUp) {
        unlisten();
      } else {
        unlistenCallbacks.push(unlisten);
      }
    };

    void listen<SpeakerDiarizationProgress>('speaker-diarization-progress', (event) => {
      if (event.payload.meeting_id !== meetingIdRef.current) return;
      observedEvent = true;
      if (event.payload.stage !== 'complete') {
        setIsDiarizing(true);
      }
      setDiarizationMessage(event.payload.message);
      setDiarizationProgress(event.payload);
    }).then(trackUnlistener);

    void listen<SpeakerDiarizationComplete>('speaker-diarization-complete', (event) => {
      if (event.payload.meeting_id !== meetingIdRef.current) return;
      observedEvent = true;
      showDiarizationComplete(event.payload);
    }).then(trackUnlistener);

    void listen<SpeakerDiarizationError>('speaker-diarization-error', (event) => {
      if (event.payload.meeting_id !== meetingIdRef.current) return;
      observedEvent = true;
      showDiarizationError(event.payload.error);
    }).then(trackUnlistener);

    void invoke<string | null>('active_speaker_diarization_command').then(active => {
      if (!cleanedUp && !observedEvent) setIsDiarizing(active === meetingIdRef.current);
    }).catch(() => {});

    return () => {
      cleanedUp = true;
      unlistenCallbacks.forEach((unlisten) => unlisten());
    };
  }, [meetingId, showDiarizationComplete, showDiarizationError]);

  const handleCancelDiarization = useCallback(async () => {
    if (!meetingId || isCancelling) return;
    setIsCancelling(true);
    setDiarizationMessage('Cancelling after the current speech step…');
    try {
      await invoke('cancel_speaker_diarization_command', { meetingId });
    } catch (error) {
      setIsCancelling(false);
      toast.error('Could not cancel speaker detection', { description: String(error) });
    }
  }, [meetingId, isCancelling]);

  const handleRunSpeakerDiarization = async (numSpeakers: number | null, preserveExistingLabels: boolean) => {
    if (!meetingId || !meetingFolderPath || isDiarizing || startingDetection.current) return;
    startingDetection.current = true;
    setDetectionRequest(null);
    lastCompletionKeyRef.current = null;
    handledErrorRef.current = false;
    const speakerMode = numSpeakers ? `${numSpeakers} speakers` : 'Auto speaker detection';
    setIsDiarizing(true);
    setIsCancelling(false);
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
      const complete = await invoke<SpeakerDiarizationComplete>('start_speaker_diarization_command', {
        meetingId,
        meetingFolderPath,
        segmentationModelPath: null,
        embeddingModelPath: null,
        embeddingModelId: null,
        numSpeakers,
        preserveExistingLabels,
      });
      showDiarizationComplete(complete);
    } catch (error) {
      showDiarizationError(String(error));
    } finally {
      startingDetection.current = false;
    }
  };

  const requestSpeakerDetection = (numSpeakers: number | null) => {
    if (hasSpeakerLabels || !speakerLabelsComplete) setDetectionRequest({ numSpeakers });
    else void handleRunSpeakerDiarization(numSpeakers, true);
  };

  const speakerDetectionDisabled = transcriptCount === 0 || isDiarizing;
  const hasRecordingFolder = Boolean(meetingFolderPath?.trim());

  return (
    <div className="flex shrink-0 items-center justify-end gap-1">
      {onFindReplace && <Button variant="ghost" size="sm" className="px-1 text-xs" disabled={correctionActions?.busy || transcriptCount === 0} onClick={onFindReplace}>Find and replace</Button>}
      <Button variant="ghost" size="sm" className="px-1 text-xs" disabled={transcriptCount === 0} onClick={() => { Analytics.trackButtonClick('copy_transcript', 'meeting_details'); onCopyTranscript(); }}>Copy</Button>
      <DropdownMenu><DropdownMenuTrigger asChild><ActionIcon ref={menuTrigger} label="More transcript actions" className="h-8 w-8"><MoreHorizontal /></ActionIcon></DropdownMenuTrigger>
        <DropdownMenuContent align="end" onCloseAutoFocus={event => { if (detectionRequest) event.preventDefault(); }}>
          {meetingId && meetingFolderPath && audioAvailable && <DropdownMenuItem title={transcriptCount === 0 ? 'Transcribe saved audio' : 'Retranscribe to enhance your recorded audio'} onClick={() => { Analytics.trackButtonClick('enhance_transcript', 'meeting_details'); onTranscribeOpenChange(true); }}>{transcriptCount === 0 ? 'Transcribe saved audio' : 'Enhance or retranscribe'}</DropdownMenuItem>}
          {onFind && <DropdownMenuItem disabled={transcriptCount === 0} onSelect={onFind}>Find in transcript<DropdownMenuShortcut>{playerKeycaps(bindings.find).join('+')}</DropdownMenuShortcut></DropdownMenuItem>}
          {onAddBookmark && <DropdownMenuItem onSelect={onAddBookmark}>Add bookmark<DropdownMenuShortcut>{playerKeycaps(bindings.bookmark).join('+')}</DropdownMenuShortcut></DropdownMenuItem>}
          {correctionActions?.canUndo && <DropdownMenuItem disabled={correctionActions.busy} onSelect={correctionActions.undo}>Undo last correction</DropdownMenuItem>}
          {correctionActions?.canRestore && <DropdownMenuItem disabled={correctionActions.busy} onSelect={correctionActions.restore}>Restore previous transcript</DropdownMenuItem>}
          {meetingId && audioAvailable && canDetectSpeakers(hasRecordingFolder, transcriptCount) && <>
            <DropdownMenuSeparator /><DropdownMenuLabel>{isDiarizing ? diarizationMessage ?? 'Detecting speakers…' : 'Detect speakers'}</DropdownMenuLabel>
            <DropdownMenuItem disabled={speakerDetectionDisabled} onSelect={() => requestSpeakerDetection(null)}>Auto detect</DropdownMenuItem>
            {[2, 3, 4, 5, 6].map(count => <DropdownMenuItem key={count} disabled={speakerDetectionDisabled} onSelect={() => requestSpeakerDetection(count)}>{count} speakers</DropdownMenuItem>)}
          </>}
          {isDiarizing && <DropdownMenuItem disabled={isCancelling} onSelect={() => void handleCancelDiarization()}>{isCancelling ? 'Cancelling…' : 'Cancel speaker detection'}</DropdownMenuItem>}
        </DropdownMenuContent>
      </DropdownMenu>


      {meetingId && meetingFolderPath && audioAvailable && (
        <RetranscribeDialog
          open={transcribeOpen}
          onOpenChange={onTranscribeOpenChange}
          meetingId={meetingId}
          meetingFolderPath={meetingFolderPath}
          hasTranscript={transcriptCount > 0}
          onComplete={handleRetranscribeComplete}
        />
      )}

      <Dialog open={detectionRequest !== null} onOpenChange={open => { if (!open) setDetectionRequest(null); }}>
        <DialogContent onCloseAutoFocus={event => {
          event.preventDefault();
          if (!startingDetection.current) menuTrigger.current?.focus();
        }}>
          <DialogHeader>
            <DialogTitle>Detect speakers</DialogTitle>
            <DialogDescription>
              {hasSpeakerLabels ? 'This transcript already has speaker labels.' : 'Some transcript segments have yet to load and may have speaker labels.'}
              {' '}Keep existing labels and fill unlabeled segments, or replace all labels with detection results.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => setDetectionRequest(null)}>Cancel</Button>
            <Button type="button" onClick={() => {
              if (detectionRequest) void handleRunSpeakerDiarization(detectionRequest.numSpeakers, true);
            }}>Keep my labels</Button>
            <Button type="button" variant="outline" onClick={() => {
              if (detectionRequest) void handleRunSpeakerDiarization(detectionRequest.numSpeakers, false);
            }}>Replace</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <SpeakerDiarizationDialog
        open={showDiarizationDialog}
        onOpenChange={setShowDiarizationDialog}
        isProcessing={isDiarizing}
        isCancelling={isCancelling}
        onCancel={() => void handleCancelDiarization()}
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
