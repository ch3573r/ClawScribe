import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { BuiltInModelManager, type BuiltInModelInfo } from '@/components/BuiltInModelManager';
import { ModelSaveFeedback } from './shared';
import type { FeedbackState } from '@/components/settings/commit-field';

export function useBuiltInModels() {
  const [models, setModels] = useState<BuiltInModelInfo[]>([]);
  const load = async () => {
    if (models.length) return;
    try { setModels(await invoke<BuiltInModelInfo[]>('builtin_ai_list_models')); }
    catch { toast.error('Could not load built-in models'); }
  };
  return { models: models.map(model => model.name), load };
}

export function BuiltInSettings({ model, chooseModel, layout, feedback }: {
  model: string; chooseModel: (model: string) => void; layout: 'inline' | 'dialog'; feedback?: FeedbackState;
}) {
  return <div className="px-5 py-3">
    <ModelSaveFeedback feedback={feedback} />
    <BuiltInModelManager selectedModel={model} layout={layout} onModelSelect={chooseModel} />
  </div>;
}
