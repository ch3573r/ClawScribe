'use client';

import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Button } from '@/components/ui/button';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { ConfirmationModal } from '@/components/ConfirmationModel/confirmation-modal';
import { Progress } from '@/components/ui/progress';
import { cn } from '@/lib/utils';
import { Download, RefreshCw, BadgeAlert, Trash2 } from 'lucide-react';
import { toast } from 'sonner';
import { formatSummaryModelSizeLabelFromMb } from '@/lib/onboarding-summary-model';

export interface BuiltInModelInfo {
  name: string;
  display_name: string;
  status: {
    type: 'not_downloaded' | 'downloading' | 'available' | 'corrupted' | 'error';
    progress?: number;
  };
  size_mb: number;
  context_size: number;
  description: string;
  gguf_file: string;
}

interface DownloadEvent {
  model: string;
  progress: number;
  downloaded_mb?: number;
  total_mb?: number;
  speed_mbps?: number;
  status: 'downloading' | 'completed' | 'cancelled' | 'error';
}

interface DownloadProgressInfo {
  downloadedMb: number;
  totalMb: number;
  speedMbps: number;
}

interface BuiltInModelManagerProps {
  selectedModel: string;
  onModelSelect: (model: string) => void;
  layout?: 'inline' | 'dialog';
}

export function BuiltInModelManager({
  selectedModel,
  onModelSelect,
  layout = 'inline',
}: BuiltInModelManagerProps) {
  const [models, setModels] = useState<BuiltInModelInfo[]>([]);
  const [isLoading, setIsLoading] = useState<boolean>(false);
  const [hasFetched, setHasFetched] = useState<boolean>(false);
  const [downloadProgress, setDownloadProgress] = useState<Record<string, number>>({});
  const [downloadProgressInfo, setDownloadProgressInfo] = useState<Record<string, DownloadProgressInfo>>({});
  const [downloadingModels, setDownloadingModels] = useState<Set<string>>(new Set());
  const [pendingDelete, setPendingDelete] = useState<BuiltInModelInfo | null>(null);
  const [isDeleting, setIsDeleting] = useState(false);
  const deleting = useRef(false);
  const deleteTrigger = useRef<HTMLElement | null>(null);
  const modelList = useRef<HTMLDivElement | null>(null);

  const fetchRevision = useRef(0);
  const fetchModels = useCallback(async () => {
    const revision = ++fetchRevision.current;
    try {
      setIsLoading(true);
      const data = await invoke<BuiltInModelInfo[]>('builtin_ai_list_models');
      if (revision !== fetchRevision.current) return;
      setModels(data);
    } catch (error) {
      if (revision !== fetchRevision.current) return;
      console.error('Failed to fetch built-in AI models:', error);
      toast.error('Failed to load models');
    } finally {
      if (revision === fetchRevision.current) {
        setIsLoading(false);
        setHasFetched(true);
      }
    }
  }, []);

  useEffect(() => {
    void fetchModels();
    return () => { fetchRevision.current += 1; };
  }, [fetchModels]);

  // Listen for download progress events
  useEffect(() => {
    const subscription = listen<DownloadEvent>('builtin-ai-download-progress', ({ payload }) => {
      const { model, progress, downloaded_mb, total_mb, speed_mbps, status } = payload;
      if (status === 'downloading') {
        // Update percentage progress
        setDownloadProgress((prev) => ({
          ...prev,
          [model]: progress,
        }));

        // Update detailed progress info (MB, speed)
        setDownloadProgressInfo((prev) => ({
          ...prev,
          [model]: {
            downloadedMb: downloaded_mb ?? 0,
            totalMb: total_mb ?? 0,
            speedMbps: speed_mbps ?? 0,
          },
        }));

        setDownloadingModels(prev => prev.has(model) ? prev : new Set(prev).add(model));
        return;
      }

      setDownloadingModels(prev => {
        const next = new Set(prev);
        next.delete(model);
        return next;
      });
      setDownloadProgress(prev => {
        const next = { ...prev };
        delete next[model];
        return next;
      });
      setDownloadProgressInfo(prev => {
        const next = { ...prev };
        delete next[model];
        return next;
      });

      if (status === 'error') {
        // The core does not persist errors. Keep this state until a retry.
        setModels(prev => prev.map(item => item.name === model ? { ...item, status: { type: 'error' } } : item));
      } else {
        void fetchModels();
        if (status === 'completed') toast.success(`Model ${model} downloaded successfully`);
      }
    });
    return () => { void subscription.then(unlisten => unlisten()); };
  }, [fetchModels]);

  const downloadModel = async (modelName: string) => {
    try {
      // Optimistically add to downloadingModels for immediate UI feedback
      setDownloadingModels((prev) => new Set([...prev, modelName]));

      await invoke('builtin_ai_download_model', { modelName });
    } catch (error) {
      console.error('Failed to download model:', error);

      // Check if this is a cancellation error (starts with "CANCELLED:")
      const errorMsg = String(error);
      if (errorMsg.startsWith('CANCELLED:')) {
        // Cancel handler already removed from downloadingModels
        // Don't show error toast for cancellations - cancel function already shows info toast
        return;
      }

      // For real errors, show toast and remove from downloading
      toast.error(`Failed to download ${modelName}`);

      setDownloadingModels((prev) => {
        const newSet = new Set(prev);
        newSet.delete(modelName);
        return newSet;
      });

      // Refresh model list to get updated Error status from backend
      fetchModels();
    }
  };

  const cancelDownload = async (modelName: string) => {
    try {
      await invoke('builtin_ai_cancel_download', { modelName });
      toast.info(`Download of ${modelName} cancelled`);
      setDownloadingModels((prev) => {
        const newSet = new Set(prev);
        newSet.delete(modelName);
        return newSet;
      });
    } catch (error) {
      console.error('Failed to cancel download:', error);
    }
  };

  const confirmDelete = async () => {
    if (!pendingDelete || deleting.current) return;
    const modelName = pendingDelete.name;
    deleting.current = true;
    setIsDeleting(true);
    try {
      await invoke('builtin_ai_delete_model', { modelName });
      toast.success(`Model ${modelName} deleted`);
      deleteTrigger.current = modelList.current;
      await fetchModels();
      setPendingDelete(null);
    } catch (error) {
      console.error('Failed to delete model:', error);
      toast.error(`Failed to delete ${modelName}`);
    } finally {
      deleting.current = false;
      setIsDeleting(false);
    }
  };

  // Don't show loading spinner if we have downloads in progress - show the model list instead
  if (isLoading && models.length === 0 && downloadingModels.size === 0) {
    return (
      <div className="text-center py-8 text-muted-foreground">
        <RefreshCw className="mx-auto h-8 w-8 animate-spin mb-2" />
        Loading models...
      </div>
    );
  }

  // Only show "no models" message after fetch has completed
  if (hasFetched && models.length === 0) {
    return (
      <Alert>
        <AlertDescription>
          No models found. Download a model to get started with Built-in AI.
        </AlertDescription>
      </Alert>
    );
  }

  return (
    <div ref={modelList} tabIndex={-1}>
      <div className="flex items-center justify-between mb-4">
        <h4 className="text-sm font-bold">Built-in AI models</h4>
      </div>

      <div
        className={cn(
          'grid gap-4',
          layout === 'dialog' && 'max-h-[50vh] overflow-y-auto pr-2 pb-2'
        )}
      >
        {models.map((model) => {
          const progress = downloadProgress[model.name];
          const progressInfo = downloadProgressInfo[model.name];
          const modelIsDownloading = downloadingModels.has(model.name);
          const isAvailable = model.status.type === 'available';
          const isNotDownloaded = model.status.type === 'not_downloaded';
          const isCorrupted = model.status.type === 'corrupted';
          const isError = model.status.type === 'error';
          const canSelect = isAvailable && !modelIsDownloading;
          const modelLabel = model.display_name || model.name;
          const requestDelete = (event: React.MouseEvent<HTMLButtonElement>) => {
            deleteTrigger.current = event.currentTarget;
            setPendingDelete(model);
          };

          return (
            <div
              key={model.name}
              className={cn(
                'relative p-4 rounded-lg border transition-colors',
                modelIsDownloading
                  ? 'bg-card border-border'
                  : 'bg-card',
                selectedModel === model.name
                  ? 'ring-2 ring-primary border-primary'
                  : 'border-border hover:border-muted-foreground'
              )}
            >
            <div className="space-y-3">
              <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                <div className="min-w-0 flex-1">
                  <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
                    {canSelect ? (
                      <Button
                        type="button"
                        variant="ghost"
                        aria-pressed={selectedModel === model.name}
                        className="h-auto min-w-0 cursor-pointer whitespace-normal break-words p-0 text-left text-base font-bold leading-snug text-foreground hover:bg-transparent focus-visible:ring-0 after:absolute after:inset-0 after:rounded-lg focus-visible:after:ring-2 focus-visible:after:ring-ring focus-visible:after:ring-offset-2 focus-visible:after:ring-offset-background"
                        onClick={() => onModelSelect(model.name)}
                      >
                        {modelLabel}
                      </Button>
                    ) : <span className="min-w-0 break-words text-base font-bold leading-snug text-foreground">{modelLabel}</span>}
                    {isAvailable && (
                      <>
                        <span className="flex shrink-0 items-center gap-1 text-xs font-medium text-success-foreground">
                          <span className="h-2 w-2 rounded-full bg-success-foreground"></span>
                          Ready
                        </span>
                        {selectedModel === model.name && (
                          <span className="shrink-0 rounded bg-primary/15 px-2 py-0.5 text-xs font-medium text-primary">
                            Selected
                          </span>
                        )}
                      </>
                    )}
                    {isCorrupted && (
                      <span className="flex shrink-0 items-center gap-1 rounded bg-destructive/10 px-2 py-0.5 text-xs font-medium text-destructive">
                        <BadgeAlert className="h-3 w-3" />
                        Corrupted
                      </span>
                    )}
                    {isError && (
                      <span className="shrink-0 rounded bg-destructive/10 px-2 py-0.5 text-xs font-medium text-destructive">
                        Error
                      </span>
                    )}
                  </div>
                </div>
                <div className="relative z-10 flex w-full shrink-0 flex-wrap items-center gap-2 empty:hidden sm:ml-4 sm:w-auto sm:justify-end">
                  {/* Not Downloaded - Show Download button */}
                  {isNotDownloaded && !modelIsDownloading && (
                    <Button
                      variant="outline"
                      size="sm"
                      className="min-w-[100px]"
                      onClick={(e) => {
                        e.stopPropagation();
                        downloadModel(model.name);
                      }}
                    >
                      <Download className="mr-2 h-4 w-4" />
                      Download
                    </Button>
                  )}
                  {/* Downloading - Show Cancel button */}
                  {modelIsDownloading && (
                    <Button
                      variant="outline"
                      size="sm"
                      className="min-w-[100px]"
                      onClick={(e) => {
                        e.stopPropagation();
                        cancelDownload(model.name);
                      }}
                    >
                      Cancel
                    </Button>
                  )}
                  {/* Error - Show Retry button */}
                  {isError && !modelIsDownloading && (
                    <Button
                      variant="outline"
                      size="sm"
                      className="min-w-[100px]"
                      onClick={(e) => {
                        e.stopPropagation();
                        downloadModel(model.name);
                      }}
                    >
                      <RefreshCw className="mr-2 h-4 w-4" />
                      Retry
                    </Button>
                  )}
                  {/* Corrupted - Show both Retry and Delete buttons */}
                  {isCorrupted && !modelIsDownloading && (
                    <>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={(e) => {
                          e.stopPropagation();
                          downloadModel(model.name);
                        }}
                      >
                        <RefreshCw className="mr-2 h-4 w-4" />
                        Retry
                      </Button>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={requestDelete}
                        aria-label={`Delete ${modelLabel}`}
                      >
                        <Trash2 className="mr-2 h-4 w-4" />
                        Delete
                      </Button>
                    </>
                  )}
                  {/* Available - Show small trash icon (only if not currently selected) */}
                  {isAvailable && !modelIsDownloading && selectedModel !== model.name && (
                    <Button type="button" variant="ghost" size="icon"
                      className="h-8 w-8 text-muted-foreground hover:text-destructive"
                      onClick={requestDelete}
                      aria-label={`Delete ${modelLabel}`}
                    >
                      <Trash2 aria-hidden="true" />
                    </Button>
                  )}
                </div>
              </div>
              <div className="text-sm text-muted-foreground">
                {model.description && (
                  <p className="mb-1">{model.description}</p>
                )}
                {(isError || isCorrupted) && (
                  <p className="mb-1 text-xs text-destructive">
                    {isError && typeof model.status === 'object' && 'Error' in model.status && typeof model.status.Error === 'string'
                      ? model.status.Error
                      : isCorrupted
                      ? 'File is corrupted. Retry download or delete.'
                      : 'An error occurred'}
                  </p>
                )}
                <div className="text-xs text-muted-foreground">
                  <span>{formatSummaryModelSizeLabelFromMb(model.size_mb)} • {model.context_size} tokens</span>
                </div>
                </div>
              </div>

              {/* Download progress bar */}
              {modelIsDownloading && progress !== undefined && (
                <div className="mt-3 pt-3 border-t border-border">
                  <div className="flex items-center justify-between mb-1">
                    <span className="text-sm font-medium text-foreground">Downloading...</span>
                    <span className="text-sm font-semibold text-foreground">
                      {Math.round(progress)}%
                    </span>
                  </div>
                  <div className="text-sm text-muted-foreground mb-2">
                    {progressInfo?.totalMb > 0 ? (
                      <>
                        {progressInfo.downloadedMb.toFixed(1)} MiB / {progressInfo.totalMb.toFixed(1)} MiB
                        {progressInfo.speedMbps > 0 && (
                          <span className="ml-2 text-muted-foreground">
                            ({progressInfo.speedMbps.toFixed(1)} MiB/s)
                          </span>
                        )}
                      </>
                    ) : (
                      <span>{formatSummaryModelSizeLabelFromMb(model.size_mb)}</span>
                    )}
                  </div>
                  <Progress value={progress} className="h-2.5" aria-label={`Download progress for ${modelLabel}`} />
                </div>
              )}
            </div>
          );
        })}
      </div>
      <ConfirmationModal
        isOpen={pendingDelete !== null}
        title="Delete model?"
        text={pendingDelete ? `This removes ${pendingDelete.display_name || pendingDelete.name} (${formatSummaryModelSizeLabelFromMb(pendingDelete.size_mb)}) from this PC. You can download it again later.` : ''}
        confirmLabel={isDeleting ? 'Deleting…' : 'Delete model'}
        confirmDisabled={isDeleting}
        returnFocus={deleteTrigger}
        onCancel={() => { if (!deleting.current) setPendingDelete(null); }}
        onConfirm={confirmDelete}
      />
    </div>
  );
}
