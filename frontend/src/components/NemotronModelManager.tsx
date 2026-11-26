import React, { useState, useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { recommendedForSpeed, usePcTestResults } from '@/components/settings/pcTestResults';
import { TranscriptionModelTable } from '@/components/settings/ParakeetModelTable';
import { SettingsCard, SettingsRow } from '@/components/ui/settings-layout';
import { formatModelSize, type ModelStatus as TableModelStatus } from '@/lib/parakeet';

type ModelStatus = Exclude<TableModelStatus, { Downloading: number }> | { Downloading: { progress: number } };

interface RawModelInfo {
  name: string;
  size_mb: number;
  status: ModelStatus;
  description?: string;
}

interface NemotronModelManagerProps {
  selectedModel?: string;
  onModelSelect?: (modelName: string) => void;
  className?: string;
}

export function NemotronModelManager({
  selectedModel,
  onModelSelect,
  className = '',
}: NemotronModelManagerProps) {
  const [models, setModels] = useState<RawModelInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const pcResults = usePcTestResults();
  const speedRecommendations = recommendedForSpeed(models, pcResults);

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
    const subscriptions = [
      listen<{ modelName: string; progress: number }>(
        'nemotron-model-download-progress',
        (e) => patchModel(e.payload.modelName, { status: { Downloading: { progress: e.payload.progress } } }),
      ),
      listen<{ modelName: string }>('nemotron-model-download-complete', (e) => {
        patchModel(e.payload.modelName, { status: 'Available' });
        toast.success('Nemotron ready', { description: 'Model downloaded and ready to use' });
      }),
      listen<{ modelName: string; error: string }>('nemotron-model-download-error', (e) => {
        patchModel(e.payload.modelName, { status: { Error: e.payload.error } });
        toast.error('Failed to download Nemotron', { description: e.payload.error });
      }),
    ];
    return () => subscriptions.forEach(subscription => void subscription.then(unlisten => unlisten()));
  }, []);

  const download = async (modelName: string) => {
    const model = models.find(item => item.name === modelName);
    if (!model) return;
    patchModel(modelName, { status: { Downloading: { progress: 0 } } });
    toast.info('Downloading Nemotron…', {
      description: `This download is about ${formatModelSize(model.size_mb)}.`,
    });
    try {
      await invoke('nemotron_download_model', { modelName });
    } catch (e) {
      patchModel(modelName, { status: { Error: e instanceof Error ? e.message : 'Download failed' } });
    }
  };

  const cancel = async (modelName: string) => {
    try {
      await invoke('nemotron_cancel_download', { modelName });
      patchModel(modelName, { status: 'Missing' });
      toast.info('Download cancelled');
    } catch (e) {
      console.error('Cancel failed:', e);
      toast.error('Failed to cancel download');
    }
  };

  const rows = models.map(model => ({
    name: model.name,
    label: model.name.includes('int8') ? 'Nemotron 3.5 ASR INT8' : 'Nemotron 3.5 ASR FP16',
    bestFor: model.description || 'Multilingual streaming transcription',
    sizeMb: model.size_mb,
    status: typeof model.status === 'object' && 'Downloading' in model.status
      ? { Downloading: model.status.Downloading.progress }
      : model.status,
    recommendation: speedRecommendations.get(model.name),
    badge: model.name.includes('int8') ? 'GPU only' : undefined,
  }));

  return (
    <SettingsCard title="Nemotron models" description="Beta" className={className} aria-busy={loading}>
      {loading ? <SettingsRow title="Loading models" role="status" />
        : error ? <SettingsRow title="Models unavailable" description={error} role="alert" />
        : rows.length === 0 ? <SettingsRow title="No Nemotron models available" />
        : <TranscriptionModelTable
          label="Nemotron models"
          models={rows}
          selectedModel={selectedModel}
          results={pcResults}
          onSelect={onModelSelect}
          onDownload={download}
          onCancel={cancel}
        />}
    </SettingsCard>
  );
}
