import React, { useState, useEffect, useRef, useCallback } from 'react';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import {
  ParakeetModelInfo,
  ModelStatus,
  ParakeetAPI,
  getModelDisplayInfo,
} from '../lib/parakeet';
import { ParakeetModelTable } from '@/components/settings/ParakeetModelTable';
import { usePcTestResults } from '@/components/settings/pcTestResults';
import { PcTestAction, PcTestReport, useTranscriptionPcTest } from '@/components/TranscriptionPcTest';
import { errorText } from '@/lib/errorText';

interface ParakeetModelManagerProps {
  selectedModel?: string;
  onModelSelect?: (modelName: string) => void;
  className?: string;
  /** Engine and model that "Test this PC" measures. Leave out to hide the test control. */
  pcTest?: { provider: string; model: string };
}

export function ParakeetModelManager({
  selectedModel,
  onModelSelect,
  className = '',
  pcTest
}: ParakeetModelManagerProps) {
  const [models, setModels] = useState<ParakeetModelInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [downloadingModels, setDownloadingModels] = useState<Set<string>>(new Set());

  const pcResults = usePcTestResults();
  // Unused (and unsupported, so disabled) when no test target is given.
  const test = useTranscriptionPcTest(pcTest ?? { provider: 'none', model: '' });

  const retryDownload = useRef<((name: string) => Promise<void>) | null>(null);

  // Progress throttle map to prevent rapid updates
  const progressThrottleRef = useRef<Map<string, { progress: number; timestamp: number }>>(new Map());

  const loadModels = useCallback(async () => {
    try {
      setLoading(true);
      setError(null);
      await ParakeetAPI.init();
      setModels(await ParakeetAPI.getAvailableModels());
    } catch (err) {
      console.error('Failed to initialize Parakeet:', err);
      setError(err instanceof Error ? err.message : 'Failed to load models');
      toast.error('Failed to load transcription models', {
        description: errorText(err),
        duration: 5000
      });
    } finally {
      setLoading(false);
    }
  }, []);

  // Load the model list when the table is shown
  useEffect(() => {
    void loadModels();
  }, [loadModels]);

  // Set up event listeners for download progress
  useEffect(() => {
    const subscriptions = [
      // Download progress with throttling
      listen<{ modelName: string; progress: number }>(
        'parakeet-model-download-progress',
        (event) => {
          const { modelName, progress } = event.payload;
          const now = Date.now();
          const throttleData = progressThrottleRef.current.get(modelName);

          // Throttle: only update if 300ms passed OR progress jumped by 5%+
          const shouldUpdate = !throttleData ||
            now - throttleData.timestamp > 300 ||
            Math.abs(progress - throttleData.progress) >= 5;

          if (shouldUpdate) {
            console.log(`[ParakeetModelManager] Progress update for ${modelName}: ${progress}%`);
            progressThrottleRef.current.set(modelName, { progress, timestamp: now });

            setModels(prevModels =>
              prevModels.map(model =>
                model.name === modelName
                  ? { ...model, status: { Downloading: progress } as ModelStatus }
                  : model
              )
            );
          }
        }
      ),

      // Download complete
      listen<{ modelName: string }>(
        'parakeet-model-download-complete',
        (event) => {
          const { modelName } = event.payload;
          const displayInfo = getModelDisplayInfo(modelName);
          const displayName = displayInfo?.friendlyName || modelName;

          setModels(prevModels =>
            prevModels.map(model =>
              model.name === modelName
                ? { ...model, status: 'Available' as ModelStatus }
                : model
            )
          );

          setDownloadingModels(prev => {
            const newSet = new Set(prev);
            newSet.delete(modelName);
            return newSet;
          });

          // Clean up throttle data
          progressThrottleRef.current.delete(modelName);

          toast.success(`${displayName} is ready`, {
            description: 'Model downloaded and ready to use',
            duration: 4000
          });
        }
      ),

      // Download error
      listen<{ modelName: string; error: string }>(
        'parakeet-model-download-error',
        (event) => {
          const { modelName, error } = event.payload;
          const displayInfo = getModelDisplayInfo(modelName);
          const displayName = displayInfo?.friendlyName || modelName;

          setModels(prevModels =>
            prevModels.map(model =>
              model.name === modelName
                ? { ...model, status: { Error: error } as ModelStatus }
                : model
            )
          );

          setDownloadingModels(prev => {
            const newSet = new Set(prev);
            newSet.delete(modelName);
            return newSet;
          });

          // Clean up throttle data
          progressThrottleRef.current.delete(modelName);

          toast.error(`Failed to download ${displayName}`, {
            description: error,
            duration: 6000,
            action: {
              label: 'Retry',
              onClick: () => { void retryDownload.current?.(modelName); }
            }
          });
        }
      ),
    ];
    return () => {
      for (const subscription of subscriptions) {
        void subscription.then(unlisten => unlisten());
      }
    };
  }, []); // Empty dependency array - listeners use refs for stable callbacks

  const cancelDownload = async (modelName: string) => {
    const displayInfo = getModelDisplayInfo(modelName);
    const displayName = displayInfo?.friendlyName || modelName;

    try {
      await ParakeetAPI.cancelDownload(modelName);

      setDownloadingModels(prev => {
        const newSet = new Set(prev);
        newSet.delete(modelName);
        return newSet;
      });

      setModels(prevModels =>
        prevModels.map(model =>
          model.name === modelName
            ? { ...model, status: 'Missing' as ModelStatus }
            : model
        )
      );

      // Clean up throttle data
      progressThrottleRef.current.delete(modelName);

      toast.info(`${displayName} download cancelled`, {
        duration: 3000
      });
    } catch (err) {
      console.error('Failed to cancel download:', err);
      toast.error('Failed to cancel download', {
        description: errorText(err),
        duration: 4000
      });
    }
  };

  const downloadModel = async (modelName: string) => {
    if (downloadingModels.has(modelName)) return;

    const displayInfo = getModelDisplayInfo(modelName);
    const displayName = displayInfo?.friendlyName || modelName;

    try {
      setDownloadingModels(prev => new Set([...prev, modelName]));

      setModels(prevModels =>
        prevModels.map(model =>
          model.name === modelName
            ? { ...model, status: { Downloading: 0 } as ModelStatus }
            : model
        )
      );

      toast.info(`Downloading ${displayName}...`, {
        description: 'This may take a few minutes',
        duration: 5000  // Auto-dismiss after 5 seconds
      });

      await ParakeetAPI.downloadModel(modelName);
    } catch (err) {
      console.error('Download failed:', err);
      setDownloadingModels(prev => {
        const newSet = new Set(prev);
        newSet.delete(modelName);
        return newSet;
      });

      const errorMessage = err instanceof Error ? err.message : 'Download failed';
      setModels(prev =>
        prev.map(model =>
          model.name === modelName ? { ...model, status: { Error: errorMessage } } : model
        )
      );
    }
  };

  useEffect(() => { retryDownload.current = downloadModel; });

  const deleteModel = async (modelName: string) => {
    const displayInfo = getModelDisplayInfo(modelName);
    const displayName = displayInfo?.friendlyName || modelName;

    try {
      await ParakeetAPI.deleteCorruptedModel(modelName);

      // Refresh models list
      const modelList = await ParakeetAPI.getAvailableModels();
      setModels(modelList);

      toast.success(`${displayName} deleted`, {
        description: 'Model removed to free up space',
        duration: 3000
      });

    } catch (err) {
      console.error('Failed to delete model:', err);
      toast.error(`Failed to delete ${displayName}`, {
        description: err instanceof Error ? err.message : 'Delete failed',
        duration: 4000
      });
    }
  };

  return (
    <div className={className}>
      <ParakeetModelTable
        models={models}
        loading={loading}
        error={error}
        selectedModel={selectedModel}
        results={pcResults}
        testAction={pcTest ? <PcTestAction test={test} /> : undefined}
        testReport={pcTest ? <PcTestReport test={test} /> : undefined}
        onSelect={onModelSelect}
        onDownload={modelName => void downloadModel(modelName)}
        onCancel={modelName => void cancelDownload(modelName)}
        onDelete={modelName => void deleteModel(modelName)}
        onRetryLoad={() => void loadModels()}
      />
    </div>
  );
}
