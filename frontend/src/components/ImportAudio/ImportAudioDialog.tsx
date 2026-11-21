import React, { useState, useEffect, useCallback, useMemo, useRef } from 'react';
import {
  Upload,
  Globe,
  Loader2,
  AlertCircle,
  CheckCircle2,
  X,
  Cpu,
  FileAudio,
  Clock,
  HardDrive,
  ChevronDown,
  ChevronUp,
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
import { Input } from '../ui/input';
import { Checkbox } from '../ui/checkbox';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '../ui/select';
import { toast } from 'sonner';
import { useConfig } from '@/contexts/ConfigContext';
import { importSummarySelection, useSummaryDestination, type ImportSummaryChoice } from '@/hooks/useSummaryDestination';
import { providerLabel } from '@/components/settings/provider-data';
import { useImportAudio, ImportResult, type AudioFileInfo } from '@/hooks/useImportAudio';
import { useRouter } from 'next/navigation';
import { invoke } from '@tauri-apps/api/core';
import { isActiveSummary, type SummaryJobSnapshot } from '@/lib/summary-job-status';
import { useSidebar } from '../Sidebar/SidebarProvider';
import { LANGUAGES } from '@/constants/languages';
import { useTranscriptionModels, ModelOption } from '@/hooks/useTranscriptionModels';


interface ImportAudioDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  preselectedFile?: string | null;
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

function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

export function ImportAudioDialog({
  open,
  onOpenChange,
  preselectedFile,
  onComplete,
}: ImportAudioDialogProps) {
  const router = useRouter();
  const { refetchMeetings } = useSidebar();
  const { selectedLanguage, transcriptModelConfig, modelConfig, isAutoSummary } = useConfig();

  const [title, setTitle] = useState('');
  const [meetingDate, setMeetingDate] = useState('');
  const [useFileDate, setUseFileDate] = useState(false);
  const { destination, error: destinationError } = useSummaryDestination(open, modelConfig, isAutoSummary);
  const [summaryChoice, setSummaryChoice] = useState<ImportSummaryChoice>('default');
  const summarySelection = importSummarySelection(summaryChoice, isAutoSummary, destination);
  const summarize = summarySelection !== null;
  const [remainingFiles, setRemainingFiles] = useState<AudioFileInfo[]>([]);
  const [selectedLang, setSelectedLang] = useState(selectedLanguage || 'auto');
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [titleModifiedByUser, setTitleModifiedByUser] = useState(false);

  // Benchmark stats captured on a successful import, shown instead of navigating
  // away immediately so the import dialog doubles as a transcription benchmark.
  const [stats, setStats] = useState<{
    meetingId: string;
    segments: number;
    audioSeconds: number;
    processingSeconds: number;
    modelLabel: string;
    summaryRequested: boolean;
  } | null>(null);
  const importStartedAtRef = useRef<number | null>(null);
  const importedModelLabelRef = useRef<string>('Default model');
  const summaryRequested = useRef(false);
  // Tracks the last preselected path we validated so a file dropped onto an
  // already-open dialog gets picked up without re-validating on every re-render.
  const lastValidatedPathRef = useRef<string | null>(null);

  // Always start as false — represents "dialog has not yet been opened".
  // Do NOT initialize from the `open` prop: if the component mounts with open=true
  // (e.g. drag-drop path), we still need the initialization effect to run.
  const prevOpenRef = useRef(false);

  // Use centralized model fetching hook
  const {
    availableModels,
    selectedModelKey,
    setSelectedModelKey,
    loadingModels,
    fetchModels,
    resetSelection,
  } = useTranscriptionModels(transcriptModelConfig);

  const handleImportComplete = useCallback((result: ImportResult) => {
    const startedAt = importStartedAtRef.current;
    const processingSeconds = startedAt ? (performance.now() - startedAt) / 1000 : 0;
    setStats({
      meetingId: result.meeting_id,
      segments: result.segments_count,
      audioSeconds: result.duration_seconds,
      processingSeconds,
      modelLabel: importedModelLabelRef.current,
      summaryRequested: summaryRequested.current,
    });
    toast.success(`Import complete — ${result.segments_count} segments created.`);

    // Keep the dialog open to show benchmark stats; refresh the list in the
    // background so the new meeting is ready when the user opens it.
    refetchMeetings();
    onComplete?.();
  }, [refetchMeetings, onComplete]);

  const handleImportError = useCallback((error: string) => {
    toast.error('Import failed', { description: error });
  }, []);

  const handleImportCancelled = useCallback(() => {
    toast.info('Import cancelled');
    onOpenChange(false);
  }, [onOpenChange]);

  const {
    status,
    fileInfo,
    progress,
    error,
    isProcessing,
    isCancelling,
    isBusy,
    selectFiles,
    chooseFile,
    validateFile,
    startImport,
    cancelImport,
    reset,
  } = useImportAudio({
    onComplete: handleImportComplete,
    onError: handleImportError,
    onCancelled: handleImportCancelled,
  });

  // Reset state only when dialog transitions from closed to open
  // This prevents re-initialization when config changes while dialog is already open (Bug #4 & #5)
  useEffect(() => {
    const wasOpen = prevOpenRef.current;
    prevOpenRef.current = open;

    // Only initialize when transitioning from closed (false) to open (true)
    if (open && !wasOpen) {
      reset();
      resetSelection();
      setTitle('');
      setMeetingDate('');
      setUseFileDate(false);
      setSummaryChoice('default');
      setRemainingFiles([]);
      setTitleModifiedByUser(false);
      setStats(null);
      setSelectedLang(selectedLanguage || 'auto');
      setShowAdvanced(false);

      // Fetch available models using centralized hook
      fetchModels();
    }
  }, [open, selectedLanguage, transcriptModelConfig, reset, resetSelection, fetchModels]);

  // Validate the preselected file. Runs both on open and whenever the path
  // changes while the dialog is already open, so dragging a new file onto an
  // open dialog loads it instead of being ignored.
  useEffect(() => {
    if (!open) {
      lastValidatedPathRef.current = null;
      return;
    }
    if (!preselectedFile || lastValidatedPathRef.current === preselectedFile || isBusy) return;
    lastValidatedPathRef.current = preselectedFile;
    setRemainingFiles([]);
    setMeetingDate('');
    setUseFileDate(false);
    setSummaryChoice('default');
    setTitleModifiedByUser(false);
    setStats(null);
    validateFile(preselectedFile).then((info) => {
      if (info) setTitle(info.filename);
    });
  }, [open, preselectedFile, validateFile, isBusy]);

  // Update title when fileInfo changes
  useEffect(() => {
    if (fileInfo && !title && !titleModifiedByUser) {
      setTitle(fileInfo.filename);
    }
  }, [fileInfo, title, titleModifiedByUser]);

  const selectedModel = useMemo((): ModelOption | undefined => {
    if (!selectedModelKey) return undefined;
    const colonIndex = selectedModelKey.indexOf(':');
    if (colonIndex === -1) return undefined;
    const provider = selectedModelKey.slice(0, colonIndex);
    const name = selectedModelKey.slice(colonIndex + 1);
    return availableModels.find((m) => m.provider === provider && m.name === name);
  }, [selectedModelKey, availableModels]);
  const isParakeetModel = selectedModel?.provider === 'parakeet';

  useEffect(() => {
    if (isParakeetModel && selectedLang !== 'auto') {
      setSelectedLang('auto');
    }
  }, [isParakeetModel, selectedLang]);

  const handleSelectFile = async () => {
    const [first, ...rest] = await selectFiles();
    if (!first) return;
    setTitle(first.filename);
    setMeetingDate('');
    setUseFileDate(false);
    setSummaryChoice('default');
    setRemainingFiles(rest);
  };

  const handleStartImport = async () => {
    if (!fileInfo || isBusy) return;
    const date = meetingDate ? new Date(meetingDate) : null;
    if (date && Number.isNaN(date.getTime())) {
      toast.error('Choose a valid meeting date and time');
      return;
    }

    importStartedAtRef.current = performance.now();
    importedModelLabelRef.current = selectedModel?.displayName || 'Default model';
    summaryRequested.current = summarize;

    await startImport(
      fileInfo.path,
      title || fileInfo.filename,
      isParakeetModel ? null : selectedLang === 'auto' ? null : selectedLang,
      selectedModel?.name || null,
      selectedModel?.provider || null,
      {
        meetingAt: date?.toISOString() ?? null,
        useFileDate,
        summarize,
        summaryDestinationFingerprint: summarySelection?.fingerprint ?? null,
        summaryUseAutomaticPreference: summarySelection?.useAutomaticPreference ?? false,
      },
    );
  };

  const handleImportAnother = async () => {
    if (stats?.summaryRequested) {
      try {
        const job = await invoke<SummaryJobSnapshot>('api_get_summary', { meetingId: stats.meetingId });
        if (isActiveSummary(job.status)) {
          toast.info('Notes are still generating for the last import', {
            description: 'Open that meeting to see progress, or continue when its notes are ready.',
          });
          return;
        }
      } catch {
        toast.error('Could not check summary status');
        return;
      }
    }
    const [next, ...rest] = remainingFiles;
    if (next) {
      chooseFile(next);
      setRemainingFiles(rest);
    } else {
      reset();
      resetSelection();
    }
    setStats(null);
    setTitle(next?.filename ?? '');
    setMeetingDate('');
    setUseFileDate(false);
    setSummaryChoice('default');
    setTitleModifiedByUser(false);
    importStartedAtRef.current = null;
  };

  const handleOpenImportedMeeting = () => {
    if (!stats) return;
    onOpenChange(false);
    router.push(`/meeting-details?id=${stats.meetingId}`);
  };

  const handleCancel = async () => {
    if (isProcessing) {
      await cancelImport();
      return;
    }
    onOpenChange(false);
  };

  // Prevent closing during processing
  const handleOpenChange = (newOpen: boolean) => {
    if (!newOpen && (isProcessing || isCancelling)) {
      return;
    }
    onOpenChange(newOpen);
  };

  const handleEscapeKeyDown = (event: KeyboardEvent) => {
    if (isProcessing || isCancelling) {
      event.preventDefault();
    }
  };

  const handleInteractOutside = (event: Event) => {
    if (isProcessing || isCancelling) {
      event.preventDefault();
    }
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        className="max-h-[calc(100dvh-2rem)] overflow-y-auto sm:max-w-[500px]"
        onEscapeKeyDown={handleEscapeKeyDown}
        onInteractOutside={handleInteractOutside}
      >
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            {isCancelling ? (
              <>
                <Loader2 className="h-5 w-5 animate-spin text-primary" />
                Cancelling import...
              </>
            ) : isProcessing ? (
              <>
                <Loader2 className="h-5 w-5 animate-spin text-primary" />
                Importing audio...
              </>
            ) : error ? (
              <>
                <AlertCircle className="h-5 w-5 text-destructive" />
                Import failed
              </>
            ) : status === 'complete' ? (
              <>
                <CheckCircle2 className="h-5 w-5 text-success-foreground" />
                Import complete
              </>
            ) : (
              <>
                <Upload className="h-5 w-5 text-primary" />
                Import audio or video
              </>
            )}
          </DialogTitle>
          <DialogDescription>
            {isCancelling
              ? 'Stopping after the current processing step finishes.'
              : isProcessing
              ? progress?.message || 'Processing audio...'
              : error
              ? 'An error occurred during import'
              : status === 'complete'
              ? stats?.summaryRequested ? 'Open the saved meeting to check summary status.' : 'Your meeting is ready.'
              : 'Create one meeting per audio or video file'}
          </DialogDescription>
        </DialogHeader>

        <div className="min-w-0 space-y-4 py-4">
          {/* File selection / info */}
          {!isProcessing && !isCancelling && !error && status !== 'complete' && (
            <>
              {fileInfo ? (
                <div className="bg-muted rounded-lg p-4 space-y-3">
                  <div className="flex items-start gap-3">
                    <FileAudio className="h-8 w-8 text-primary flex-shrink-0" />
                    <div className="flex-1 min-w-0">
                      <p className="font-medium text-foreground truncate">{fileInfo.filename}</p>
                      <div className="flex items-center gap-4 text-sm text-muted-foreground mt-1">
                        <span className="flex items-center gap-1">
                          <Clock className="h-3.5 w-3.5" />
                          {fileInfo.duration_seconds > 0 ? formatDuration(fileInfo.duration_seconds) : "Measured during import"}
                        </span>
                        <span className="flex items-center gap-1">
                          <HardDrive className="h-3.5 w-3.5" />
                          {formatFileSize(fileInfo.size_bytes)}
                        </span>
                        <span className="text-primary font-medium">{fileInfo.format}</span>
                      </div>
                    </div>
                  </div>

                  {/* Editable title */}
                  <div className="space-y-1">
                    <label htmlFor="import-title" className="text-sm font-medium text-muted-foreground">Meeting title</label>
                    <Input
                      id="import-title"
                      value={title}
                      onChange={(e) => {
                        setTitle(e.target.value);
                        setTitleModifiedByUser(true);
                      }}
                      placeholder="Enter meeting title"
                    />
                  </div>

                  <div className="space-y-1">
                    <label htmlFor="import-meeting-date" className="text-sm font-medium text-muted-foreground">Meeting date and time (local)</label>
                    <Input id="import-meeting-date" type="datetime-local" value={meetingDate} disabled={useFileDate}
                      onChange={event => setMeetingDate(event.target.value)} aria-describedby="import-date-help" />
                    <p id="import-date-help" className="text-xs text-muted-foreground">
                      {useFileDate && fileInfo.modified_at ? 'Using the file’s modified time, ' + new Date(fileInfo.modified_at).toLocaleString() : 'Uses the import time if left blank.'}
                    </p>
                    {fileInfo.modified_at && (
                      <Button type="button" variant="ghost" size="sm" className="h-auto px-0"
                        onClick={() => setUseFileDate(value => !value)}>
                        {useFileDate ? 'Choose another date' : 'Use file modified time'}
                      </Button>
                    )}
                  </div>
                  <div className="flex items-start gap-2">
                    <Checkbox id="import-summarize" checked={summarize}
                      disabled={!destination}
                      onCheckedChange={value => setSummaryChoice(value === true && destination ? { fingerprint: destination.fingerprint } : 'off')}
                      aria-describedby="import-summary-help" />
                    <div>
                      <label htmlFor="import-summarize" className="text-sm font-medium">Generate a summary after transcription</label>
                      <p id="import-summary-help" className="break-all text-xs text-muted-foreground">
                        {destination ? destination.external
                          ? 'Sends the transcript and notes context, including the meeting date and available details, to ' + providerLabel(destination.provider) + ' (' + destination.endpoint + ') using ' + destination.model + '.'
                          : 'Generates notes locally with ' + providerLabel(destination.provider) + ' using ' + destination.model + '.'
                          : destinationError ?? 'Checking the selected summary provider…'}
                        {destination?.external && destination.unencrypted && ' This connection uses unencrypted HTTP.'}
                        {summarySelection?.useAutomaticPreference && ' Selected by your automatic notes setting. You can clear it for this file.'}
                      </p>
                    </div>
                  </div>
                  {remainingFiles.length > 0 && <p className="text-xs text-muted-foreground">
                    {remainingFiles.length} more {remainingFiles.length === 1 ? 'file' : 'files'} selected. Review each one after this import.
                  </p>}

                  <Button variant="outline" size="sm" onClick={handleSelectFile} className="w-full">
                    Choose different files
                  </Button>
                </div>
              ) : (
                <div className="border-2 border-dashed border-border rounded-lg p-8 text-center">
                  <FileAudio className="h-12 w-12 text-muted-foreground mx-auto mb-4" />
                  <Button onClick={handleSelectFile} disabled={status === 'validating'}>
                    {status === 'validating' ? (
                      <>
                        <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                        Validating...
                      </>
                    ) : (
                      <>
                        <Upload className="h-4 w-4 mr-2" />
                        Select audio files
                      </>
                    )}
                  </Button>
                  <p className="text-sm text-muted-foreground mt-2">MP4, WAV, MP3, FLAC, OGG, MKV, WebM, WMA</p>
                </div>
              )}

              {/* Advanced options (collapsible) */}
              {fileInfo && (
                <div className="border rounded-lg">
                  <Button variant="ghost"
                    onClick={() => setShowAdvanced(!showAdvanced)}
                    className="w-full flex items-center justify-between p-3 text-sm font-medium text-muted-foreground hover:bg-muted h-auto rounded-none hover:text-muted-foreground"
                  >
                    <span>Advanced options</span>
                    {showAdvanced ? (
                      <ChevronUp className="h-4 w-4" />
                    ) : (
                      <ChevronDown className="h-4 w-4" />
                    )}
                  </Button>

                  {showAdvanced && (
                    <div className="p-3 pt-0 space-y-4 border-t">
                      {/* Language selector */}
                      {!isParakeetModel ? (
                        <div className="space-y-2">
                          <div className="flex items-center gap-2">
                            <Globe className="h-4 w-4 text-muted-foreground" />
                            <span className="text-sm font-medium">Language</span>
                          </div>
                          <Select value={selectedLang} onValueChange={setSelectedLang}>
                            <SelectTrigger className="w-full">
                              <SelectValue placeholder="Select language" />
                            </SelectTrigger>
                            <SelectContent className="max-h-60">
                              {LANGUAGES.map((lang) => (
                                <SelectItem key={lang.code} value={lang.code}>
                                  {lang.name}
                                </SelectItem>
                              ))}
                            </SelectContent>
                          </Select>
                        </div>
                      ) : (
                        <div className="space-y-2">
                          <div className="flex items-center gap-2">
                            <Globe className="h-4 w-4 text-muted-foreground" />
                            <span className="text-sm font-medium">Language</span>
                          </div>
                          <p className="text-xs text-muted-foreground">
                            Language selection isn&apos;t supported for Parakeet. It always uses automatic detection.
                          </p>
                        </div>
                      )}

                      {/* Model selector */}
                      {availableModels.length > 0 && (
                        <div className="space-y-2">
                          <div className="flex items-center gap-2">
                            <Cpu className="h-4 w-4 text-muted-foreground" />
                            <span className="text-sm font-medium">Model</span>
                          </div>
                          <Select
                            value={selectedModelKey}
                            onValueChange={setSelectedModelKey}
                            disabled={loadingModels}
                          >
                            <SelectTrigger className="w-full">
                              <SelectValue placeholder={loadingModels ? 'Loading models...' : 'Select model'} />
                            </SelectTrigger>
                            <SelectContent>
                              {availableModels.map((model) => (
                                <SelectItem
                                  key={`${model.provider}:${model.name}`}
                                  value={`${model.provider}:${model.name}`}
                                >
                                  {model.displayName} ({Math.round(model.size_mb)} MB)
                                </SelectItem>
                              ))}
                            </SelectContent>
                          </Select>
                        </div>
                      )}
                    </div>
                  )}
                </div>
              )}
            </>
          )}

          {/* Progress display */}
          {(isProcessing || isCancelling) && progress && (
            <div className="space-y-2">
              <div className="relative">
                <div className="w-full bg-muted rounded-full h-3">
                  <div
                    className="bg-primary h-3 rounded-full transition-all duration-300 ease-out"
                    style={{ width: `${Math.min(progress.progress_percentage, 100)}%` }}
                  />
                </div>
                <div className="flex justify-between text-xs text-muted-foreground mt-1">
                  <span>{progress.stage}</span>
                  <span>{Math.round(progress.progress_percentage)}%</span>
                </div>
              </div>
              <p className="text-sm text-muted-foreground text-center">{progress.message}</p>
            </div>
          )}

          {/* Error display */}
          {error && (
            <div className="bg-destructive/10 border border-destructive/30 rounded-lg p-3">
              <p className="text-sm text-destructive">{error}</p>
            </div>
          )}

          {/* Benchmark stats (shown on success) */}
          {status === 'complete' && stats && (
            <div className="space-y-3">
              <div className="grid grid-cols-2 gap-3">
                <div className="rounded-lg border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Clock className="h-3.5 w-3.5" />
                    Audio length
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">
                    {formatDuration(stats.audioSeconds)}
                  </p>
                </div>
                <div className="rounded-lg border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Gauge className="h-3.5 w-3.5" />
                    Processing time
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">
                    {stats.processingSeconds.toFixed(1)}s
                  </p>
                </div>
                <div className="rounded-lg border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Zap className="h-3.5 w-3.5" />
                    Speed
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">
                    {stats.processingSeconds > 0
                      ? `${(stats.audioSeconds / stats.processingSeconds).toFixed(1)}× realtime`
                      : '—'}
                  </p>
                </div>
                <div className="rounded-lg border border-border bg-muted p-3">
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Hash className="h-3.5 w-3.5" />
                    Segments
                  </div>
                  <p className="mt-1 text-lg font-semibold text-foreground">{stats.segments}</p>
                </div>
              </div>
              <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                <Cpu className="h-3.5 w-3.5" />
                Transcribed with {stats.modelLabel}
              </div>
            </div>
          )}
        </div>

        <DialogFooter>
          {!isProcessing && !isCancelling && !error && status !== 'complete' && (
            <>
              <Button variant="outline" onClick={() => onOpenChange(false)}>
                Cancel
              </Button>
              <Button
                onClick={handleStartImport}
                className="bg-primary hover:bg-primary/90"
                disabled={!fileInfo || isBusy}
              >
                <Upload className="h-4 w-4 mr-2" />
                {summarize ? 'Import and summarize' : 'Import'}
              </Button>
            </>
          )}
          {status === 'complete' && (
            <>
              <Button variant="outline" onClick={handleImportAnother}>
                {remainingFiles.length > 0 ? 'Review next file' : 'Import another'}
              </Button>
              <Button onClick={handleOpenImportedMeeting} className="bg-primary hover:bg-primary/90">
                Open meeting
              </Button>
            </>
          )}
          {(isProcessing || isCancelling) && (
            <Button variant="outline" onClick={handleCancel} disabled={isCancelling}>
              {isCancelling ? (
                <Loader2 className="h-4 w-4 mr-2 animate-spin" />
              ) : (
                <X className="h-4 w-4 mr-2" />
              )}
              {isCancelling ? 'Cancelling...' : 'Cancel'}
            </Button>
          )}
          {error && (
            <>
              <Button variant="outline" onClick={() => onOpenChange(false)}>
                Close
              </Button>
              <Button onClick={reset} variant="outline">
                Try again
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
