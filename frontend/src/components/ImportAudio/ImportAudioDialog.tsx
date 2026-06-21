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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '../ui/select';
import { toast } from 'sonner';
import { useConfig } from '@/contexts/ConfigContext';
import { useImportAudio, ImportResult } from '@/hooks/useImportAudio';
import { useRouter } from 'next/navigation';
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
  const { selectedLanguage, transcriptModelConfig } = useConfig();

  const [title, setTitle] = useState('');
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
  } | null>(null);
  const importStartedAtRef = useRef<number | null>(null);
  const importedModelLabelRef = useRef<string>('Default model');
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
    selectFile,
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
    if (!preselectedFile || lastValidatedPathRef.current === preselectedFile) return;
    lastValidatedPathRef.current = preselectedFile;
    setTitleModifiedByUser(false);
    setStats(null);
    validateFile(preselectedFile).then((info) => {
      if (info) setTitle(info.filename);
    });
  }, [open, preselectedFile, validateFile]);

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
    const info = await selectFile();
    if (info) {
      setTitle(info.filename);
    }
  };

  const handleStartImport = async () => {
    if (!fileInfo) return;

    importStartedAtRef.current = performance.now();
    importedModelLabelRef.current = selectedModel?.displayName || 'Default model';

    await startImport(
      fileInfo.path,
      title || fileInfo.filename,
      isParakeetModel ? null : selectedLang === 'auto' ? null : selectedLang,
      selectedModel?.name || null,
      selectedModel?.provider || null
    );
  };

  const handleImportAnother = () => {
    reset();
    resetSelection();
    setStats(null);
    setTitle('');
    setTitleModifiedByUser(false);
    importStartedAtRef.current = null;
    lastValidatedPathRef.current = null;
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
        className="sm:max-w-[500px]"
        onEscapeKeyDown={handleEscapeKeyDown}
        onInteractOutside={handleInteractOutside}
      >
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            {isCancelling ? (
              <>
                <Loader2 className="h-5 w-5 animate-spin text-primary" />
                Cancelling Import...
              </>
            ) : isProcessing ? (
              <>
                <Loader2 className="h-5 w-5 animate-spin text-primary" />
                Importing Audio...
              </>
            ) : error ? (
              <>
                <AlertCircle className="h-5 w-5 text-destructive" />
                Import Failed
              </>
            ) : status === 'complete' ? (
              <>
                <CheckCircle2 className="h-5 w-5 text-emerald-500" />
                Import Complete
              </>
            ) : (
              <>
                <Upload className="h-5 w-5 text-primary" />
                Import Audio File
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
              ? 'Your meeting is ready. Transcription benchmark below.'
              : 'Import an audio file to create a new meeting with transcripts'}
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
                          {formatDuration(fileInfo.duration_seconds)}
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
                    <label className="text-sm font-medium text-muted-foreground">Meeting Title</label>
                    <Input
                      value={title}
                      onChange={(e) => {
                        setTitle(e.target.value);
                        setTitleModifiedByUser(true);
                      }}
                      placeholder="Enter meeting title"
                    />
                  </div>

                  <Button variant="outline" size="sm" onClick={handleSelectFile} className="w-full">
                    Choose Different File
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
                        Select Audio File
                      </>
                    )}
                  </Button>
                  <p className="text-sm text-muted-foreground mt-2">MP4, WAV, MP3, FLAC, OGG, MKV, WebM, WMA</p>
                </div>
              )}

              {/* Advanced options (collapsible) */}
              {fileInfo && (
                <div className="border rounded-lg">
                  <button
                    onClick={() => setShowAdvanced(!showAdvanced)}
                    className="w-full flex items-center justify-between p-3 text-sm font-medium text-muted-foreground hover:bg-muted"
                  >
                    <span>Advanced Options</span>
                    {showAdvanced ? (
                      <ChevronUp className="h-4 w-4" />
                    ) : (
                      <ChevronDown className="h-4 w-4" />
                    )}
                  </button>

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
                            Language selection isn't supported for Parakeet. It always uses automatic detection.
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
                disabled={!fileInfo}
              >
                <Upload className="h-4 w-4 mr-2" />
                Import
              </Button>
            </>
          )}
          {status === 'complete' && (
            <>
              <Button variant="outline" onClick={handleImportAnother}>
                Import Another
              </Button>
              <Button onClick={handleOpenImportedMeeting} className="bg-primary hover:bg-primary/90">
                Open Meeting
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
                Try Again
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
