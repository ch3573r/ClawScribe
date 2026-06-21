import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import Analytics from '@/lib/analytics';
import { applyPinnedSummaryLanguageToMeeting } from '@/lib/summary-language-preferences';
import { toast } from 'sonner';

export interface AudioFileInfo {
  path: string;
  filename: string;
  duration_seconds: number;
  size_bytes: number;
  format: string;
}

export interface ImportProgress {
  stage: string;
  progress_percentage: number;
  message: string;
}

export interface ImportResult {
  meeting_id: string;
  title: string;
  segments_count: number;
  duration_seconds: number;
}

export interface ImportError {
  error: string;
}

export type ImportStatus = 'idle' | 'validating' | 'processing' | 'cancelling' | 'complete' | 'error';

export interface UseImportAudioOptions {
  onComplete?: (result: ImportResult) => void;
  onError?: (error: string) => void;
  onCancelled?: () => void;
}

export interface UseImportAudioReturn {
  status: ImportStatus;
  fileInfo: AudioFileInfo | null;
  progress: ImportProgress | null;
  error: string | null;
  isProcessing: boolean;
  isCancelling: boolean;
  isBusy: boolean;
  selectFile: () => Promise<AudioFileInfo | null>;
  validateFile: (path: string) => Promise<AudioFileInfo | null>;
  startImport: (
    sourcePath: string,
    title: string,
    language?: string | null,
    model?: string | null,
    provider?: string | null
  ) => Promise<void>;
  cancelImport: () => Promise<void>;
  reset: () => void;
}

export function useImportAudio({
  onComplete,
  onError,
  onCancelled,
}: UseImportAudioOptions = {}): UseImportAudioReturn {
  const [status, setStatus] = useState<ImportStatus>('idle');
  const [fileInfo, setFileInfo] = useState<AudioFileInfo | null>(null);
  const [progress, setProgress] = useState<ImportProgress | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Stable refs for callbacks to avoid listener re-registration on every render
  const onCompleteRef = useRef(onComplete);
  const onErrorRef = useRef(onError);
  const onCancelledRef = useRef(onCancelled);
  useEffect(() => { onCompleteRef.current = onComplete; }, [onComplete]);
  useEffect(() => { onErrorRef.current = onError; }, [onError]);
  useEffect(() => { onCancelledRef.current = onCancelled; }, [onCancelled]);

  // Cancellation guard: prevents late events from updating state after cancel
  const isCancelledRef = useRef(false);

  // Set up event listeners (registered once, use refs for callbacks)
  useEffect(() => {
    const unlisteners: UnlistenFn[] = [];
    const cleanedUpRef = { current: false };

    const setupListeners = async () => {
      // Progress events
      const unlistenProgress = await listen<ImportProgress>(
        'import-progress',
        (event) => {
          if (isCancelledRef.current) return;
          setProgress(event.payload);
          setStatus('processing');
        }
      );
      if (cleanedUpRef.current) {
        unlistenProgress();
        return;
      }
      unlisteners.push(unlistenProgress);

      // Completion event
      const unlistenComplete = await listen<ImportResult>(
        'import-complete',
        async (event) => {
          isCancelledRef.current = false;

          await Analytics.track('import_audio_completed', {
            success: 'true',
            duration_seconds: event.payload.duration_seconds.toString(),
            segments_count: event.payload.segments_count.toString()
          });

          setStatus('complete');
          setProgress(null);
          try {
            await applyPinnedSummaryLanguageToMeeting(event.payload.meeting_id);
          } catch (error) {
            console.warn('Failed to apply pinned summary language to imported meeting:', error);
            toast.warning('Could not apply default summary language', {
              description: 'The imported meeting was saved, but the default summary language was not applied.',
            });
          }
          onCompleteRef.current?.(event.payload);
        }
      );
      if (cleanedUpRef.current) {
        unlistenComplete();
        unlisteners.forEach(u => u());
        return;
      }
      unlisteners.push(unlistenComplete);

      // Error event
      const unlistenError = await listen<ImportError>(
        'import-error',
        async (event) => {
          const wasCancelling = isCancelledRef.current;
          const isCancellation = event.payload.error.toLowerCase().includes('cancelled');

          if (wasCancelling && isCancellation) {
            isCancelledRef.current = false;
            setStatus('idle');
            setProgress(null);
            setError(null);
            await Analytics.track('import_audio_cancelled', { success: 'true' });
            onCancelledRef.current?.();
            return;
          }

          isCancelledRef.current = false;

          await Analytics.trackError('import_audio_failed', event.payload.error);

          setStatus('error');
          setError(event.payload.error);
          onErrorRef.current?.(event.payload.error);
        }
      );
      if (cleanedUpRef.current) {
        unlistenError();
        unlisteners.forEach(u => u());
        return;
      }
      unlisteners.push(unlistenError);
    };

    setupListeners();

    return () => {
      cleanedUpRef.current = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, []);

  // Select file using native file dialog
  const selectFile = useCallback(async (): Promise<AudioFileInfo | null> => {
    setStatus('validating');
    setError(null);

    try {
      const result = await invoke<AudioFileInfo | null>('select_and_validate_audio_command');
      if (result) {
        setFileInfo(result);
        setStatus('idle');
        return result;
      } else {
        // User cancelled
        setStatus('idle');
        return null;
      }
    } catch (err: any) {
      setStatus('error');
      const errorMsg = typeof err === 'string' ? err : (err?.message || String(err) || 'Failed to validate file');
      setError(errorMsg);
      onErrorRef.current?.(errorMsg);
      return null;
    }
  }, []);

  // Validate a file from a given path (for drag-drop)
  const validateFile = useCallback(async (path: string): Promise<AudioFileInfo | null> => {
    setStatus('validating');
    setError(null);

    try {
      const result = await invoke<AudioFileInfo>('validate_audio_file_command', { path });
      setFileInfo(result);
      setStatus('idle');
      return result;
    } catch (err: any) {
      setStatus('error');
      const errorMsg = typeof err === 'string' ? err : (err?.message || String(err) || 'Failed to validate file');
      setError(errorMsg);
      onErrorRef.current?.(errorMsg);
      return null;
    }
  }, []);

  // Start the import process
  const startImport = useCallback(
    async (
      sourcePath: string,
      title: string,
      language?: string | null,
      model?: string | null,
      provider?: string | null
    ) => {
      isCancelledRef.current = false;
      setStatus('processing');
      setError(null);
      setProgress(null);

      try {
        if (fileInfo) {
          await Analytics.track('import_audio_started', {
            file_size_bytes: fileInfo.size_bytes.toString(),
            duration_seconds: fileInfo.duration_seconds.toString(),
            language: language || 'auto',
            model_provider: provider || '',
            model_name: model || ''
          });
        }

        await invoke('start_import_audio_command', {
          sourcePath,
          title,
          language: language || null,
          model: model || null,
          provider: provider || null,
        });
      } catch (err: any) {
        setStatus('error');
        const errorMsg = typeof err === 'string' ? err : (err?.message || String(err) || 'Failed to start import');
        setError(errorMsg);

        await Analytics.trackError('import_audio_failed', errorMsg);

        onErrorRef.current?.(errorMsg);
      }
    },
    [fileInfo]
  );

  // Cancel ongoing import
  const cancelImport = useCallback(async () => {
    isCancelledRef.current = true;
    setStatus('cancelling');
    setError(null);
    setProgress((current) => ({
      stage: 'cancelling',
      progress_percentage: current?.progress_percentage ?? 0,
      message: 'Cancelling import...'
    }));
    try {
      await invoke('cancel_import_command');
    } catch (err: any) {
      isCancelledRef.current = false;
      setStatus('error');
      setProgress(null);
      const errorMsg = typeof err === 'string' ? err : (err?.message || String(err) || 'Failed to cancel import');
      setError(errorMsg);
      console.error('Failed to cancel import:', err);
    }
  }, []);

  // Reset all state
  const reset = useCallback(() => {
    isCancelledRef.current = false;
    setStatus('idle');
    setFileInfo(null);
    setProgress(null);
    setError(null);
  }, []);

  return {
    status,
    fileInfo,
    progress,
    error,
    isProcessing: status === 'processing',
    isCancelling: status === 'cancelling',
    isBusy: status === 'processing' || status === 'validating' || status === 'cancelling',
    selectFile,
    validateFile,
    startImport,
    cancelImport,
    reset,
  };
}
