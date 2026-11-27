import React, { useState, useEffect, useRef } from 'react';
import {
  RefreshCw,
  Globe,
  Loader2,
  AlertCircle,
  CheckCircle2,
  X,
  Cpu,
  Clock,
  Gauge,
  Zap,
  Hash,
} from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '../ui/dialog';
import { Button } from '../ui/button';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '../ui/select';
import { invoke } from '@tauri-apps/api/core';
import Link from 'next/link';
import { useTranscriptionConfig } from '@/contexts/ConfigContext';
import { fileLanguageOptions, fileLanguageSelection } from '@/lib/transcription-language';
import { isCloudModel, modelLabel, modelOptionLabel, useFileCloudDestination, useTranscriptionModels } from '@/hooks/useTranscriptionModels';
import { Checkbox } from '../ui/checkbox';
import { Progress } from '../ui/progress';
import { useRetranscription } from '@/contexts/RetranscriptionProvider';
import { errorText } from '@/lib/errorText';

interface RetranscribeDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  meetingId: string;
  meetingFolderPath: string | null;
  hasTranscript: boolean;
  onComplete?: () => void;
}

function formatDuration(seconds: number): string {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = Math.floor(seconds % 60);

  if (hours > 0) {
    return `${hours}:${minutes.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
  }
  return `${minutes}:${secs.toString().padStart(2, '0')}`;
}

export function RetranscribeDialog({
  open,
  onOpenChange,
  meetingId,
  meetingFolderPath,
  hasTranscript,
  onComplete,
}: RetranscribeDialogProps) {
  const { selectedLanguage, transcriptModelConfig } = useTranscriptionConfig();
  const { status, starting, cancellingRunId, ready, connectionError, reconnect, start, cancel } = useRetranscription();
  const [dismissedRunId, setDismissedRunId] = useState<number | null>(null);
  const [admissionError, setAdmissionError] = useState<{ meetingId: string; message: string } | null>(null);
  const job = status?.meeting_id === meetingId && status.run_id !== dismissedRunId && status.state !== 'cancelled' ? status : null;
  const ownStart = starting?.meetingId === meetingId ? starting : null;
  const running = job?.state === 'running' ? job : null;
  const progress = running?.progress;
  const isProcessing = Boolean(running || ownStart);
  const isCancelling = running !== null && running.run_id === cancellingRunId;
  const isRetranscription = ownStart?.replacesTranscript ?? job?.replaces_transcript ?? hasTranscript;
  const error = isProcessing ? null : admissionError?.meetingId === meetingId ? admissionError.message
    : job?.state === 'failed' ? job.error : connectionError;
  const completed = job?.state === 'complete' ? job : null;
  const stats = completed ? {
    segments: completed.result.segments_count,
    audioSeconds: completed.result.duration_seconds,
    processingSeconds: completed.processing_seconds,
    modelLabel: modelLabel(completed.result.provider, completed.result.model),
    language: completed.result.language || (completed.result.provider === 'nemotron' ? 'System language' : 'Auto detect'),
    fileWarning: completed.result.file_warning,
    fallbackReason: completed.result.fallback_reason,
  } : null;
  const [hasEdits, setHasEdits] = useState(false);
  const [editStateError, setEditStateError] = useState(false);
  useEffect(() => {
    if (!open || !hasTranscript || !isRetranscription) return;
    let disposed = false;
    setHasEdits(false); setEditStateError(false);
    void invoke<{has_edits: boolean}>('api_get_transcript_edit_state', { meetingId })
      .then(state => { if (!disposed) setHasEdits(state.has_edits); })
      .catch(() => { if (!disposed) setEditStateError(true); });
    return () => { disposed = true; };
  }, [open, meetingId, hasTranscript, isRetranscription]);
  const [selectedLang, setSelectedLang] = useState(selectedLanguage || 'auto');

  // Use centralized model fetching hook
  const {
    availableModels,
    selectedModelKey,
    setSelectedModelKey,
    loadingModels,
    fetchModels,
    resetSelection,
  } = useTranscriptionModels(transcriptModelConfig);

  const selectedModelDetails = availableModels.find(model => `${model.provider}:${model.name}` === selectedModelKey);
  const provider = selectedModelDetails?.provider;
  const isParakeetModel = provider === 'parakeet';
  const effectiveLanguage = fileLanguageSelection(provider, selectedLang);
  const cloudProvider = isCloudModel(provider) ? provider : undefined;
  const { destination, destinationError, approvedDestination, setApprovedDestination } =
    useFileCloudDestination(open && !isProcessing && !error && !stats, cloudProvider, meetingId);

  const onCompleteRef = useRef(onComplete);
  useEffect(() => { onCompleteRef.current = onComplete; }, [onComplete]);
  const completeRunId = completed?.run_id;
  useEffect(() => {
    if (completeRunId !== undefined) onCompleteRef.current?.();
  }, [completeRunId]);

  useEffect(() => {
    if (!loadingModels && provider && selectedLang !== effectiveLanguage) setSelectedLang(effectiveLanguage);
  }, [loadingModels, provider, selectedLang, effectiveLanguage]);

  const previousDialog = useRef({ open: false, meetingId });
  useEffect(() => {
    const previous = previousDialog.current;
    previousDialog.current = { open, meetingId };
    if (!open || (previous.open && previous.meetingId === meetingId)) return;
    resetSelection();
    setAdmissionError(null);
    setSelectedLang(selectedLanguage || 'auto');
    void fetchModels();
  }, [open, meetingId, selectedLanguage, fetchModels, resetSelection]);

  const anotherMeetingBusy = (status?.state === 'running' && status.meeting_id !== meetingId)
    || (starting !== null && starting.meetingId !== meetingId);
  const canStart = Boolean(meetingFolderPath && selectedModelDetails)
    && ready && !loadingModels && !isProcessing && !anotherMeetingBusy
    && (!cloudProvider || Boolean(approvedDestination));

  const handleStartRetranscription = async () => {
    if (!canStart || !selectedModelDetails || !meetingFolderPath) return;
    setAdmissionError(null);
    try {
      await start({
        meetingId,
        meetingFolderPath,
        replacesTranscript: hasTranscript,
        language: effectiveLanguage === 'auto' ? null : effectiveLanguage,
        model: selectedModelDetails.name,
        provider: selectedModelDetails.provider,
        cloudDestination: cloudProvider ? approvedDestination : null,
      });
    } catch (caught) {
      setAdmissionError({ meetingId, message: errorText(caught, 'Could not start transcription.') });
    }
  };

  function clearOutcome() {
    if (job && job.state !== 'running') setDismissedRunId(job.run_id);
    setAdmissionError(null);
  }

  function handleOpenChange(nextOpen: boolean) {
    if (!nextOpen && !isProcessing) clearOutcome();
    onOpenChange(nextOpen);
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-[450px]">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            {isCancelling ? (
              <>
                <Loader2 className="h-5 w-5 animate-spin text-primary" />
                {isRetranscription ? 'Cancelling retranscription...' : 'Cancelling transcription...'}
              </>
            ) : isProcessing ? (
              <>
                <Loader2 className="h-5 w-5 animate-spin text-primary" />
                {isRetranscription ? 'Retranscribing...' : 'Transcribing…'}
              </>
            ) : error ? (
              <>
                <AlertCircle className="h-5 w-5 text-destructive" />
                {isRetranscription ? 'Retranscription failed' : 'Transcription failed'}
              </>
            ) : stats ? (
              <>
                <CheckCircle2 className="h-5 w-5 text-success-foreground" />
                {isRetranscription ? 'Retranscription complete' : 'Transcription complete'}
              </>
            ) : (
              <>
                <RefreshCw className="h-5 w-5 text-primary" />
                {isRetranscription ? 'Retranscribe meeting' : 'Transcribe meeting'}
              </>
            )}
          </DialogTitle>
          <DialogDescription>
            {isCancelling
              ? 'Stopping after the current processing step finishes.'
              : isProcessing
              ? progress?.message || 'Processing audio...'
              : error
                ? (isRetranscription ? 'An error occurred during retranscription' : 'An error occurred during transcription')
                : stats
                  ? (isRetranscription ? 'Updated transcript is ready. Benchmark below.' : 'Your transcript is ready. Benchmark below.')
                : (isRetranscription ? 'Replace the current transcript using the saved audio' : 'Create a transcript from the saved audio')}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-4">
          {stats?.fileWarning && (
            <p role="alert" className="text-sm text-warning-foreground">
              The recording-folder copy needs updating. Use Retry file update below the transcript.
            </p>
          )}
          {isRetranscription && !isProcessing && !isCancelling && !error && !stats && (
            <div
              className="rounded-lg border p-3 text-sm"
              style={{
                backgroundColor: 'hsl(var(--theme-warning-bg))',
                borderColor: 'hsl(var(--theme-warning-fg) / 0.35)',
                color: 'hsl(var(--theme-warning-fg))',
              }}
            >
              <div className="flex gap-2">
                <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
                <div>
                  <p className="font-medium">Retranscription replaces the current transcript.</p>
                  {hasEdits && <p role="alert" className="mt-1">Your corrections will be replaced. You can restore the previous transcript afterwards. Summary source links will need regenerating.</p>}
                  {editStateError && <p role="alert" className="mt-1">Could not check for corrections. Any corrections will be replaced; you can restore the previous transcript afterwards.</p>}
                  <p className="mt-1 text-xs leading-5 opacity-90">
                    The existing transcript remains available until the replacement
                    finishes successfully. Meeting audio, notes, and summaries are not deleted.
                  </p>
                </div>
              </div>
            </div>
          )}

          {!isProcessing && !isCancelling && !error && !stats && !loadingModels && selectedModelDetails && (
            !isParakeetModel ? (
              <div className="space-y-3">
                <div className="flex items-center gap-2">
                  <Globe className="h-4 w-4 text-muted-foreground" />
                  <label htmlFor="retranscription-language" className="text-sm font-medium">Language</label>
                </div>
                <Select value={effectiveLanguage} onValueChange={setSelectedLang}>
                  <SelectTrigger id="retranscription-language" className="w-full">
                    <SelectValue placeholder="Select language" />
                  </SelectTrigger>
                  <SelectContent className="max-h-60">
                    {fileLanguageOptions(selectedModelDetails?.provider).map((lang) => (
                      <SelectItem key={lang.code} value={lang.code}>
                        {lang.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            ) : (
              <div className="space-y-3">
                <div className="flex items-center gap-2">
                  <Globe className="h-4 w-4 text-muted-foreground" />
                  <span className="text-sm font-medium">Language</span>
                </div>
                <p className="text-xs text-muted-foreground">
                  Language selection isn&apos;t supported for Parakeet. It always uses automatic detection.
                </p>
              </div>
            )
          )}

          {!isProcessing && !error && !stats && anotherMeetingBusy && (
            <p role="status" className="text-sm text-muted-foreground">Transcription is running for another meeting.</p>
          )}

          {!isProcessing && !isCancelling && !error && !stats && (
            <div className="space-y-3">
              <div className="flex items-center gap-2">
                <Cpu className="h-4 w-4 text-muted-foreground" />
                <label htmlFor="retranscription-model" className="text-sm font-medium">Model</label>
              </div>
              <Select value={selectedModelKey} onValueChange={setSelectedModelKey} disabled={loadingModels || availableModels.length === 0}>
                <SelectTrigger id="retranscription-model" className="w-full">
                  <SelectValue placeholder={loadingModels ? 'Loading models…' : 'Select a model'} />
                </SelectTrigger>
                <SelectContent>
                  {availableModels.map((model) => (
                    <SelectItem key={`${model.provider}:${model.name}`} value={`${model.provider}:${model.name}`}>
                      {modelOptionLabel(model)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {!loadingModels && availableModels.length === 0 && <p role="status" className="text-xs text-muted-foreground">
                No transcription models available. <Link className="underline" href="/settings?tab=transcription">Open transcription settings</Link>
              </p>}
            </div>
          )}

          {!isProcessing && !error && !stats && cloudProvider && (
            <div className="rounded-md border border-border p-3 text-sm">
              {destination ? (
                <label className="flex items-start gap-2">
                  <Checkbox checked={approvedDestination === destination.fingerprint}
                    onCheckedChange={checked => setApprovedDestination(checked === true ? destination.fingerprint : null)} />
                  <span className="break-all">Allow this meeting’s saved audio to upload to {destination.endpoint} for transcription.</span>
                </label>
              ) : <p role={destinationError ? 'alert' : 'status'}>{destinationError ?? 'Checking the cloud transcription destination…'}</p>}
            </div>
          )}

          {(isProcessing || isCancelling) && progress && (
            <Progress value={progress.progress_percentage} aria-label="Transcription progress" />
          )}

          {error && (
            <div className="bg-error border border-error-border/25 rounded-lg p-3">
              <p role="alert" className="text-sm text-error-foreground">{error}</p>
            </div>
          )}

          {stats && !isProcessing && !isCancelling && !error && (
            <div className="space-y-3">
              <div className="grid grid-cols-2 gap-3">
                <div className="rounded-md border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Clock className="h-3.5 w-3.5" />
                    Audio length
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">
                    {formatDuration(stats.audioSeconds)}
                  </p>
                </div>
                <div className="rounded-md border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Gauge className="h-3.5 w-3.5" />
                    Processing time
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">
                    {stats.processingSeconds.toFixed(1)}s
                  </p>
                </div>
                <div className="rounded-md border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Zap className="h-3.5 w-3.5" />
                    Speed
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">
                    {stats.processingSeconds > 0
                      ? `${(stats.audioSeconds / stats.processingSeconds).toFixed(1)}x realtime`
                      : '-'}
                  </p>
                </div>
                <div className="rounded-md border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Hash className="h-3.5 w-3.5" />
                    Segments
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">{stats.segments}</p>
                </div>
              </div>
              {stats.fallbackReason && <p className="text-sm text-warning-foreground">{stats.fallbackReason}</p>}
              <div className="space-y-1 text-xs text-muted-foreground">
                <div className="flex items-center gap-1.5">
                  <Cpu className="h-3.5 w-3.5" />
                  {isRetranscription ? 'Retranscribed' : 'Transcribed'} with {stats.modelLabel}
                </div>
                <div className="flex items-center gap-1.5">
                  <Globe className="h-3.5 w-3.5" />
                  Language: {stats.language}
                </div>
              </div>
            </div>
          )}
        </div>

        <DialogFooter>
          {!isProcessing && !isCancelling && !error && !stats && (
            <>
              <Button variant="outline" onClick={() => handleOpenChange(false)}>
                Cancel
              </Button>
              <Button
                onClick={handleStartRetranscription}
                className="bg-primary hover:bg-primary"
                disabled={!canStart}
              >
                <RefreshCw className="h-4 w-4 mr-2" />
                {isRetranscription ? 'Replace transcript' : 'Start transcription'}
              </Button>
            </>
          )}
          {stats && !isProcessing && !isCancelling && !error && (
            <>
              <Button variant="outline" onClick={() => handleOpenChange(false)}>
                Close
              </Button>
              <Button
                onClick={clearOutcome}
                variant="outline"
              >
                Retranscribe again
              </Button>
            </>
          )}
          {(isProcessing || isCancelling) && (
            <>
              <Button variant="ghost" onClick={() => { if (running) void cancel(running.run_id); }}
                disabled={!running || isCancelling || progress?.stage === 'saving' || progress?.stage === 'complete'}>
                {isCancelling ? (
                  <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                ) : (
                  <X className="h-4 w-4 mr-2" />
                )}
                {isCancelling ? 'Cancelling…' : 'Cancel'}
              </Button>
              <Button variant="outline" onClick={() => handleOpenChange(false)}>Run in background</Button>
            </>
          )}
          {error && (
            <>
              <Button variant="outline" onClick={() => handleOpenChange(false)}>
                Close
              </Button>
              <Button
                onClick={() => {
                  clearOutcome();
                  if (connectionError) reconnect();
                }}
                variant="outline"
              >
                Try again
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
