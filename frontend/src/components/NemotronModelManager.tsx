import React, { useState, useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { motion, AnimatePresence } from 'framer-motion';
import { toast } from 'sonner';
import { Loader2, CheckCircle2, Download, X } from 'lucide-react';

type ModelStatus =
  | 'Available'
  | 'Missing'
  | { Downloading: { progress: number } }
  | { Error: string };

interface RawModelInfo {
  name: string;
  size_mb: number;
  status: ModelStatus;
  description?: string;
}

interface NemotronModelManagerProps {
  selectedModel?: string;
  onModelSelect?: (modelName: string) => void;
  autoSave?: boolean;
  className?: string;
}

/** Short variant tag from the model id (the two exports share a display name). */
function variantTag(name: string): string {
  return name.includes('int8') ? 'INT8 · GPU only' : 'FP16';
}

function gbLabel(sizeMb: number): string {
  return sizeMb >= 1024 ? `${(sizeMb / 1024).toFixed(1)} GB` : `${sizeMb} MB`;
}

/**
 * Settings model manager for the Nemotron streaming engine (Beta). Lists every
 * downloadable variant (fp16 — CPU-capable; int8 — smaller, GPU-only). Downloads
 * and selection mirror the Parakeet manager; the UI is token-colored for dark mode
 * and self-contained (no lib/nemotron layer).
 */
export function NemotronModelManager({
  selectedModel,
  onModelSelect,
  autoSave = false,
  className = '',
}: NemotronModelManagerProps) {
  const [models, setModels] = useState<RawModelInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const onSelectRef = useRef(onModelSelect);
  const autoSaveRef = useRef(autoSave);
  useEffect(() => {
    onSelectRef.current = onModelSelect;
    autoSaveRef.current = autoSave;
  }, [onModelSelect, autoSave]);

  const saveSelection = async (name: string) => {
    try {
      await invoke('api_save_transcript_config', { provider: 'nemotron', model: name, apiKey: null });
    } catch (e) {
      console.error('Failed to save Nemotron selection:', e);
    }
  };

  // Patch one model in the list by name, leaving the others untouched.
  const patchModel = (name: string, patch: Partial<RawModelInfo>) =>
    setModels((list) => list.map((m) => (m.name === name ? { ...m, ...patch } : m)));

  useEffect(() => {
    let active = true;
    (async () => {
      try {
        setLoading(true);
        await invoke('nemotron_init');
        const list = await invoke<RawModelInfo[]>('nemotron_get_available_models');
        if (active) setModels(list);
      } catch (e) {
        if (active) setError(e instanceof Error ? e.message : 'Failed to load models');
      } finally {
        if (active) setLoading(false);
      }
    })();
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    (async () => {
      unlisteners.push(
        await listen<{ modelName: string; progress: number }>(
          'nemotron-model-download-progress',
          (e) => patchModel(e.payload.modelName, { status: { Downloading: { progress: e.payload.progress } } }),
        ),
      );
      unlisteners.push(
        await listen<{ modelName: string }>('nemotron-model-download-complete', (e) => {
          patchModel(e.payload.modelName, { status: 'Available' });
          toast.success('🌊 Nemotron ready!', { description: 'Model downloaded and ready to use' });
          if (onSelectRef.current) {
            onSelectRef.current(e.payload.modelName);
            if (autoSaveRef.current) saveSelection(e.payload.modelName);
          }
        }),
      );
      unlisteners.push(
        await listen<{ modelName: string; error: string }>('nemotron-model-download-error', (e) => {
          patchModel(e.payload.modelName, { status: { Error: e.payload.error } });
          toast.error('Failed to download Nemotron', { description: e.payload.error });
        }),
      );
    })();
    return () => unlisteners.forEach((u) => u());
  }, []);

  const download = async (model: RawModelInfo) => {
    patchModel(model.name, { status: { Downloading: { progress: 0 } } });
    toast.info('Downloading Nemotron…', {
      description: `About ${gbLabel(model.size_mb)} — this may take a few minutes`,
    });
    try {
      await invoke('nemotron_download_model', { modelName: model.name });
    } catch (e) {
      patchModel(model.name, { status: { Error: e instanceof Error ? e.message : 'Download failed' } });
    }
  };

  const cancel = async (model: RawModelInfo) => {
    try {
      await invoke('nemotron_cancel_download', { modelName: model.name });
      patchModel(model.name, { status: 'Missing' });
      toast.info('Download cancelled');
    } catch (e) {
      console.error('Cancel failed:', e);
    }
  };

  const select = async (model: RawModelInfo) => {
    onModelSelect?.(model.name);
    if (autoSave) await saveSelection(model.name);
    toast.success(`Switched to Nemotron ${variantTag(model.name)}`);
  };

  if (loading) {
    return (
      <div className={`space-y-3 ${className}`}>
        <div className="h-24 animate-pulse rounded-lg bg-muted" />
        <div className="h-24 animate-pulse rounded-lg bg-muted" />
      </div>
    );
  }
  if (error || models.length === 0) {
    return (
      <div className={`rounded-lg border border-destructive/30 bg-destructive/10 p-4 ${className}`}>
        <p className="text-sm text-destructive">Failed to load Nemotron models</p>
        {error && <p className="mt-1 text-xs text-destructive/80">{error}</p>}
      </div>
    );
  }

  return (
    <div className={`space-y-3 ${className}`}>
      {models.map((model) => {
        const isAvailable = model.status === 'Available';
        const isMissing = model.status === 'Missing';
        const isError = typeof model.status === 'object' && 'Error' in model.status;
        const progress =
          typeof model.status === 'object' && 'Downloading' in model.status
            ? model.status.Downloading.progress
            : null;
        const isSelected = selectedModel === model.name;

        return (
          <div
            key={model.name}
            className={`relative rounded-lg border-2 p-4 transition-all ${
              isSelected && isAvailable ? 'border-primary bg-primary/5' : 'border-border bg-card'
            } ${isAvailable ? 'cursor-pointer' : ''}`}
            onClick={() => isAvailable && select(model)}
          >
            <div className="flex items-start justify-between gap-4">
              <div className="flex-1">
                <div className="mb-1 flex items-center gap-2">
                  <span className="text-2xl">🌊</span>
                  <h3 className="font-semibold text-foreground">Nemotron 3.5 ASR</h3>
                  <span className="rounded-full bg-muted px-2 py-0.5 text-xs font-medium text-muted-foreground">
                    {variantTag(model.name)}
                  </span>
                  <span className="rounded-full bg-yellow-100 px-2 py-0.5 text-xs font-medium text-yellow-800">
                    BETA
                  </span>
                  {isSelected && isAvailable && (
                    <span className="flex items-center gap-1 rounded-full bg-primary px-2 py-0.5 text-xs font-medium text-primary-foreground">
                      <CheckCircle2 className="h-3 w-3" />
                    </span>
                  )}
                </div>
                <p className="ml-9 text-sm text-muted-foreground">
                  {model.description ??
                    'Streaming, multilingual (incl. German). Tries GPU (DirectML).'}{' '}
                  ~{gbLabel(model.size_mb)}.
                </p>
              </div>

              <div className="flex items-center gap-2">
                {isAvailable && (
                  <div className="flex items-center gap-1.5 text-emerald-500">
                    <div className="h-2 w-2 rounded-full bg-emerald-500" />
                    <span className="text-xs font-medium">Ready</span>
                  </div>
                )}
                {isMissing && (
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      download(model);
                    }}
                    className="flex items-center gap-1.5 rounded-md bg-primary px-3 py-1.5 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90"
                  >
                    <Download className="h-4 w-4" />
                    Download
                  </button>
                )}
                {progress === null && isError && (
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      download(model);
                    }}
                    className="rounded-md bg-destructive px-3 py-1.5 text-sm font-medium text-destructive-foreground transition-colors hover:bg-destructive/90"
                  >
                    Retry
                  </button>
                )}
              </div>
            </div>

            <AnimatePresence>
              {progress !== null && (
                <motion.div
                  initial={{ opacity: 0, height: 0 }}
                  animate={{ opacity: 1, height: 'auto' }}
                  exit={{ opacity: 0, height: 0 }}
                  className="mt-3 border-t border-border pt-3"
                >
                  <div className="mb-2 flex items-center justify-between">
                    <div className="flex items-center gap-2 text-primary">
                      <Loader2 className="h-4 w-4 animate-spin" />
                      <span className="text-sm font-medium">Downloading…</span>
                      <span className="text-sm font-semibold">{Math.round(progress)}%</span>
                    </div>
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        cancel(model);
                      }}
                      className="flex items-center gap-1 rounded px-2 py-1 text-xs font-medium text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive"
                    >
                      <X className="h-3 w-3" />
                      Cancel
                    </button>
                  </div>
                  <div className="h-2 w-full overflow-hidden rounded-full bg-muted">
                    <motion.div
                      className="h-full rounded-full bg-primary"
                      initial={{ width: 0 }}
                      animate={{ width: `${progress}%` }}
                      transition={{ duration: 0.3, ease: 'easeOut' }}
                    />
                  </div>
                </motion.div>
              )}
            </AnimatePresence>
          </div>
        );
      })}
    </div>
  );
}
