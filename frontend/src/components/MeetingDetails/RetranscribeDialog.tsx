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
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { useTranscriptionConfig } from '@/contexts/ConfigContext';
import { fileLanguageOptions, fileLanguageSelection } from '@/lib/transcription-language';
import { isCloudModel, modelLabel, modelOptionLabel, useFileCloudDestination, useTranscriptionModels } from '@/hooks/useTranscriptionModels';
import { Checkbox } from '../ui/checkbox';
import { Progress } from '../ui/progress';
import Analytics from '@/lib/analytics';

interface RetranscribeDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  meetingId: string;
  meetingFolderPath: string | null;
  hasTranscript: boolean;
  onComplete?: () => void;
}

interface RetranscriptionProgress {
  meeting_id: string;
  stage: string;
  progress_percentage: number;
  message: string;
}

interface RetranscriptionResult {
  meeting_id: string;
  segments_count: number;
  duration_seconds: number;
  language: string | null;
  provider: string;
  model: string;
  fallback_reason?: string;
  file_warning?: boolean;
}

interface RetranscriptionError {
  meeting_id: string;
  error: string;
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
  // Keep completion wording tied to this run when the parent refreshes its transcript count.
  const [startedWithTranscript, setStartedWithTranscript] = useState<boolean | null>(null);
  const isRetranscription = startedWithTranscript ?? hasTranscript;
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
  const [isProcessing, setIsProcessing] = useState(false);
  const [isCancelling, setIsCancelling] = useState(false);
  const [progress, setProgress] = useState<RetranscriptionProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selectedLang, setSelectedLang] = useState(selectedLanguage || 'auto');
  const [stats, setStats] = useState<{
    segments: number;
    audioSeconds: number;
    processingSeconds: number;
    modelLabel: string;
    language: string;
    fileWarning: boolean;
    fallbackReason?: string;
  } | null>(null);

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
  const onOpenChangeRef = useRef(onOpenChange);
  const openRef = useRef(open);
  const runRef = useRef<{ startedAt: number; replacement: boolean } | null>(null);
  const backgrounded = useRef(false);
  useEffect(() => { onCompleteRef.current = onComplete; }, [onComplete]);
  useEffect(() => { onOpenChangeRef.current = onOpenChange; }, [onOpenChange]);
  useEffect(() => { openRef.current = open; }, [open]);

  useEffect(() => {
    if (!loadingModels && provider && selectedLang !== effectiveLanguage) setSelectedLang(effectiveLanguage);
  }, [loadingModels, provider, selectedLang, effectiveLanguage]);

  const previousDialog = useRef({ open: false, meetingId });
  useEffect(() => {
    const previous = previousDialog.current;
    previousDialog.current = { open, meetingId };
    if (previous.meetingId !== meetingId) {
      runRef.current = null;
      backgrounded.current = false;
      setIsProcessing(false);
      setIsCancelling(false);
    } else if (!open || previous.open) {
      return;
    } else if (runRef.current || backgrounded.current) {
      backgrounded.current = false;
      return;
    }
    setStartedWithTranscript(null);
    resetSelection();
    setProgress(null);
    setError(null);
    setStats(null);
    setSelectedLang(selectedLanguage || 'auto');
    if (open) void fetchModels();
  }, [open, meetingId, selectedLanguage, fetchModels, resetSelection]);

  const shouldListen = open || isProcessing;
  const [listenersMeeting, setListenersMeeting] = useState<string | null>(null);
  const [listenerAttempt, setListenerAttempt] = useState(0);
  useEffect(() => {
    setListenersMeeting(null);
    if (!shouldListen) return;

    let disposed = false;
    const registrations = [
      listen<RetranscriptionProgress>('retranscription-progress', ({ payload }) => {
        if (!disposed && runRef.current && payload.meeting_id === meetingId) setProgress(payload);
      }),
      listen<RetranscriptionResult>('retranscription-complete', ({ payload }) => {
        const run = runRef.current;
        if (disposed || !run || payload.meeting_id !== meetingId) return;
        runRef.current = null;
        setIsProcessing(false);
        setIsCancelling(false);
        setProgress(null);
        setError(null);
        setStats({
          segments: payload.segments_count,
          audioSeconds: payload.duration_seconds,
          processingSeconds: (globalThis.performance.now() - run.startedAt) / 1000,
          modelLabel: modelLabel(payload.provider, payload.model),
          language: payload.language || (payload.provider === 'nemotron' ? 'System language' : 'Auto detect'),
          fileWarning: payload.file_warning === true,
          fallbackReason: payload.fallback_reason,
        });
        void Analytics.track('enhance_transcript_completed', {
          success: 'true',
          duration_seconds: payload.duration_seconds.toString(),
          segments_count: payload.segments_count.toString(),
        });
        toast.success(`${run.replacement ? 'Retranscription' : 'Transcription'} complete! ${payload.segments_count} segments created.`);
        onCompleteRef.current?.();
      }),
      listen<RetranscriptionError>('retranscription-error', ({ payload }) => {
        const run = runRef.current;
        if (disposed || !run || payload.meeting_id !== meetingId) return;
        runRef.current = null;
        setIsProcessing(false);
        setIsCancelling(false);
        setProgress(null);
        const action = run.replacement ? 'Retranscription' : 'Transcription';
        if (payload.error.toLowerCase().includes('cancelled')) {
          toast.info(`${action} cancelled`);
          onOpenChangeRef.current(false);
        } else {
          setError(payload.error);
          if (!openRef.current) toast.error(`${action} failed`, { description: payload.error });
          void Analytics.trackError('enhance_transcript_failed', payload.error);
        }
      }),
    ];
    void Promise.allSettled(registrations).then(results => {
      if (disposed) return;
      if (results.some(result => result.status === 'rejected')) {
        setError('Could not listen for transcription updates. Try again.');
      } else {
        setListenersMeeting(meetingId);
      }
    });
    return () => {
      disposed = true;
      registrations.forEach(registration => { void registration.then(unlisten => unlisten(), () => {}); });
    };
  }, [shouldListen, meetingId, listenerAttempt]);

  const canStart = Boolean(meetingFolderPath && selectedModelDetails)
    && !loadingModels && listenersMeeting === meetingId
    && (!cloudProvider || Boolean(approvedDestination));

  const handleStartRetranscription = async () => {
    if (!canStart || !selectedModelDetails || runRef.current) return;
    const run = { startedAt: globalThis.performance.now(), replacement: hasTranscript };
    runRef.current = run;
    setStartedWithTranscript(hasTranscript);
    setIsProcessing(true);
    setIsCancelling(false);
    setError(null);
    setProgress(null);
    setStats(null);

    try {
      await invoke('start_retranscription_command', {
        meetingId,
        meetingFolderPath,
        language: effectiveLanguage === 'auto' ? null : effectiveLanguage,
        model: selectedModelDetails.name,
        provider: selectedModelDetails.provider,
        cloudDestination: cloudProvider ? approvedDestination : null,
      });
      void Analytics.track('enhance_transcript_started', {
        language: effectiveLanguage,
        model_provider: selectedModelDetails.provider,
        model_name: selectedModelDetails.name,
      });
    } catch (err) {
      if (runRef.current !== run) return;
      runRef.current = null;
      setIsProcessing(false);
      setIsCancelling(false);
      const errorMsg = err instanceof Error ? err.message : String(err);
      setError(errorMsg);
      if (!openRef.current) toast.error(errorMsg);
      void Analytics.trackError('enhance_transcript_failed', errorMsg);
    }
  };

  const handleCancel = async () => {
    const run = runRef.current;
    if (run && isProcessing && !isCancelling && progress?.stage !== 'saving') {
      setIsCancelling(true);
      try {
        const accepted = await invoke<boolean>('cancel_retranscription_command');
        if (runRef.current === run && !accepted) setIsCancelling(false);
      } catch {
        if (runRef.current !== run) return;
        setIsCancelling(false);
        toast.error('Could not cancel transcription. Try again.');
      }
    }
  };

  function handleOpenChange(nextOpen: boolean) {
    if (!nextOpen && runRef.current) backgrounded.current = true;
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
              <Button variant="outline" onClick={() => onOpenChange(false)}>
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
              <Button variant="outline" onClick={() => onOpenChange(false)}>
                Close
              </Button>
              <Button
                onClick={() => {
                  setStartedWithTranscript(null);
                  setStats(null);
                  setProgress(null);
                  setError(null);
                }}
                variant="outline"
              >
                Retranscribe again
              </Button>
            </>
          )}
          {(isProcessing || isCancelling) && (
            <>
              <Button variant="ghost" onClick={handleCancel} disabled={isCancelling || progress?.stage === 'saving'}>
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
              <Button variant="outline" onClick={() => onOpenChange(false)}>
                Close
              </Button>
              <Button
                onClick={() => {
                  setError(null);
                  setProgress(null);
                  if (listenersMeeting !== meetingId) setListenerAttempt(attempt => attempt + 1);
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
