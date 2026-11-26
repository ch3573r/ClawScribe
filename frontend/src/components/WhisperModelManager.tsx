import React, { useState, useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import {
  ModelInfo,
  ModelStatus,
  getWhisperModelDisplayName,
  getModelPerformanceBadge,
  isQuantizedModel,
  getModelTagline,
  WhisperAPI
} from '../lib/whisper';
import { DEFAULT_WHISPER_MODEL } from '@/constants/modelDefaults';
import { recommendedForSpeed, usePcTestResults } from '@/components/settings/pcTestResults';
import { Accordion, AccordionContent, AccordionItem, AccordionTrigger } from '@/components/ui/accordion';
import { ConfirmationModal } from '@/components/ConfirmationModel/confirmation-modal';
import { SettingsCard, SettingsRow } from '@/components/ui/settings-layout';
import { TranscriptionModelTable } from '@/components/settings/ParakeetModelTable';

interface ModelManagerProps {
  selectedModel?: string;
  onModelSelect?: (modelName: string) => void;
  className?: string;
}

export function ModelManager({
  selectedModel,
  onModelSelect,
  className = ''
}: ModelManagerProps) {
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [initialized, setInitialized] = useState(false);
  const [downloadingModels, setDownloadingModels] = useState<Set<string>>(new Set());
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);
  const [isDeleting, setIsDeleting] = useState(false);
  const deleting = useRef(false);
  const deleteTrigger = useRef<HTMLElement | null>(null);
  const modelList = useRef<HTMLDivElement | null>(null);

  const retryDownload = useRef<((name: string) => Promise<void>) | null>(null);

  const pcResults = usePcTestResults();

  // Progress throttle map to prevent rapid updates
  const progressThrottleRef = useRef<Map<string, { progress: number; timestamp: number }>>(new Map());

  // Load persisted downloading state from localStorage
  const getPersistedDownloadingModels = (): Set<string> => {
    try {
      const saved = localStorage.getItem('downloading-models');
      return saved ? new Set<string>(JSON.parse(saved) as string[]) : new Set<string>();
    } catch {
      return new Set<string>();
    }
  };

  // Persist downloading state to localStorage
  const updateDownloadingModels = (updater: (prev: Set<string>) => Set<string>) => {
    setDownloadingModels(prev => {
      const newSet = updater(prev);
      localStorage.setItem('downloading-models', JSON.stringify(Array.from(newSet)));
      return newSet;
    });
  };

  // Initialize models
  useEffect(() => {
    if (initialized) return;

    const initializeModels = async () => {
      try {
        setLoading(true);
        await WhisperAPI.init();
        const modelList = await WhisperAPI.getAvailableModels();

        // Apply persisted downloading states
        const persistedDownloading = getPersistedDownloadingModels();
        const modelsWithDownloadState = modelList.map(model => {
          if (persistedDownloading.has(model.name) && model.status !== 'Available') {
            if (typeof model.status === 'object' && 'Corrupted' in model.status) {
              updateDownloadingModels(prev => {
                const newSet = new Set(prev);
                newSet.delete(model.name);
                return newSet;
              });
              return model;
            } else if (model.status === 'Missing') {
              updateDownloadingModels(prev => {
                const newSet = new Set(prev);
                newSet.delete(model.name);
                return newSet;
              });
              return model;
            } else {
              return { ...model, status: { Downloading: 0 } as ModelStatus };
            }
          }
          return model;
        });

        setModels(modelsWithDownloadState);
        setInitialized(true);
      } catch (err) {
        console.error('Failed to initialize Whisper:', err);
        setError(err instanceof Error ? err.message : 'Failed to load models');
        toast.error('Failed to load transcription models', {
          description: err instanceof Error ? err.message : 'Unknown error',
          duration: 5000
        });
      } finally {
        setLoading(false);
      }
    };

    initializeModels();
  }, [initialized, selectedModel, onModelSelect]);

  // Set up event listeners for download progress
  useEffect(() => {
    const subscriptions = [
      // Download progress with throttling
      listen<{ modelName: string; progress: number }>(
        'model-download-progress',
        (event) => {
          const { modelName, progress } = event.payload;
          const now = Date.now();
          const throttleData = progressThrottleRef.current.get(modelName);

          // Throttle: only update if 300ms passed OR progress jumped by 5%+
          const shouldUpdate = !throttleData ||
            now - throttleData.timestamp > 300 ||
            Math.abs(progress - throttleData.progress) >= 5;

          if (shouldUpdate) {
            console.log(`[ModelManager] Progress update for ${modelName}: ${progress}%`);
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
        'model-download-complete',
        (event) => {
          const { modelName } = event.payload;
          const displayName = getWhisperModelDisplayName(modelName);

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

          toast.success(`${displayName} ready`, {
            description: 'Model downloaded and ready to use',
            duration: 4000
          });
        }
      ),

      // Download error
      listen<{ modelName: string; error: string }>(
        'model-download-error',
        (event) => {
          const { modelName, error } = event.payload;
          const displayName = getWhisperModelDisplayName(modelName);

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
    const displayName = getWhisperModelDisplayName(modelName);

    try {
      await WhisperAPI.cancelDownload(modelName);

      updateDownloadingModels(prev => {
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
        description: err instanceof Error ? err.message : 'Unknown error',
        duration: 4000
      });
    }
  };

  const downloadModel = async (modelName: string) => {
    if (downloadingModels.has(modelName)) return;

    const displayName = getWhisperModelDisplayName(modelName);

    try {
      updateDownloadingModels(prev => new Set([...prev, modelName]));

      setModels(prevModels =>
        prevModels.map(model =>
          model.name === modelName
            ? { ...model, status: { Downloading: 0 } as ModelStatus }
            : model
        )
      );

      toast.info(`Downloading ${displayName}...`, {
        description: 'This may take a few minutes',
        duration: 5000
      });

      await WhisperAPI.downloadModel(modelName);
    } catch (err) {
      console.error('Download failed:', err);
      updateDownloadingModels(prev => {
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

  const confirmDelete = async () => {
    if (!pendingDelete || deleting.current) return;
    const modelName = pendingDelete;
    const displayName = getWhisperModelDisplayName(modelName);
    deleting.current = true;
    setIsDeleting(true);

    try {
      await WhisperAPI.deleteCorruptedModel(modelName);

      deleteTrigger.current = modelList.current;
      setModels(list => list.map(model => model.name === modelName ? { ...model, status: 'Missing' } : model));
      setPendingDelete(null);

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
    } finally {
      deleting.current = false;
      setIsDeleting(false);
    }
  };

  const basicModelNames = ['small', 'medium-q5_0', 'large-v3-q5_0', 'large-v3-turbo', 'large-v3'];
  const speedRecommendations = recommendedForSpeed(models, pcResults);
  const rows = models.map(model => ({
    name: model.name,
    label: getWhisperModelDisplayName(model.name),
    bestFor: getModelTagline(model.name, model.speed, model.accuracy),
    sizeMb: model.size_mb,
    status: model.status,
    recommendation: speedRecommendations.get(model.name) ?? (model.name === DEFAULT_WHISPER_MODEL ? 'Recommended starting model' : undefined),
    badge: isQuantizedModel(model.name) ? getModelPerformanceBadge(model.name).label : undefined,
  }));
  const basicModels = rows.filter(model => basicModelNames.includes(model.name))
    .sort((a, b) => basicModelNames.indexOf(a.name) - basicModelNames.indexOf(b.name));
  const advancedModels = rows.filter(model => !basicModelNames.includes(model.name));
  const tableProps = {
    selectedModel,
    results: pcResults,
    onSelect: onModelSelect,
    onDownload: downloadModel,
    onCancel: cancelDownload,
    onDelete: (name: string, trigger: HTMLButtonElement | null) => {
      deleteTrigger.current = trigger;
      setPendingDelete(name);
    },
  };

  return (
    <div className={className} ref={modelList} tabIndex={-1}>
      <SettingsCard title="Whisper models" aria-busy={loading}>
        {loading ? <SettingsRow title="Loading models" role="status" />
          : error ? <SettingsRow title="Models unavailable" description={error} role="alert" />
          : models.length === 0 ? <SettingsRow title="No Whisper models available" />
          : <>
            {basicModels.length > 0 && <TranscriptionModelTable label="Whisper models" models={basicModels} {...tableProps} />}
            {advancedModels.length > 0 && (
              <Accordion type="single" collapsible>
                <AccordionItem value="advanced-models" className="border-b-0">
                  <AccordionTrigger className="px-5">Advanced models</AccordionTrigger>
                  <AccordionContent className="pb-0">
                    <TranscriptionModelTable label="Advanced Whisper models" models={advancedModels} {...tableProps} />
                  </AccordionContent>
                </AccordionItem>
              </Accordion>
            )}
          </>}
      </SettingsCard>
      <ConfirmationModal
        isOpen={pendingDelete !== null}
        title="Delete model?"
        text={pendingDelete ? `This removes ${getWhisperModelDisplayName(pendingDelete)} from this PC. You can download it again later.` : ''}
        confirmLabel={isDeleting ? 'Deleting…' : 'Delete model'}
        confirmDisabled={isDeleting}
        returnFocus={deleteTrigger}
        onCancel={() => { if (!deleting.current) setPendingDelete(null); }}
        onConfirm={confirmDelete}
      />
    </div>
  );
}
