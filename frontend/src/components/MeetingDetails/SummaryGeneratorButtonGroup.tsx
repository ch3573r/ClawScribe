"use client";

import { openExternal } from '@/lib/openExternal';
import { ModelConfig, ModelSettingsModal } from '@/components/ModelSettingsModal';
import {
  Dialog,
  DialogContent,
  DialogTitle,
} from "@/components/ui/dialog"
import { VisuallyHidden } from "@/components/ui/visually-hidden"
import { Button } from '@/components/ui/button';
import { ActionIcon } from './ActionIcon';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger, DropdownMenuSub, DropdownMenuSubTrigger, DropdownMenuSubContent, DropdownMenuSeparator,
} from '@/components/ui/dropdown-menu';
import { Sparkles, Settings, Loader2, FileText, Check, Square, MoreHorizontal } from 'lucide-react';
import Analytics from '@/lib/analytics';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { useState, useEffect, ReactNode } from 'react';
import { isOllamaNotInstalledError } from '@/lib/utils';
import { BuiltInModelInfo } from '@/lib/builtin-ai';

interface SummaryGeneratorButtonGroupProps {
  onActionsCloseAutoFocus?: (event: Event) => void;
  languageSlot?: ReactNode;
  actionsSlot?: ReactNode;
  compact?: boolean;
  modelConfig: ModelConfig;
  setModelConfig: (config: ModelConfig | ((prev: ModelConfig) => ModelConfig)) => void;
  onSaveModelConfig: (config?: ModelConfig) => Promise<void>;
  onGenerateSummary: (customPrompt: string) => Promise<void>;
  onStopGeneration: () => void;
  customPrompt: string;
  summaryStatus: 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error';
  availableTemplates: Array<{ id: string, name: string, description: string }>;
  selectedTemplate: string;
  onTemplateSelect: (templateId: string, templateName: string) => void;
  hasTranscripts?: boolean;
  hasSummary?: boolean;
  isModelConfigLoading?: boolean;
  onOpenModelSettings?: (openFn: () => void) => void;
}

export function SummaryGeneratorButtonGroup({
  modelConfig,
  setModelConfig,
  onSaveModelConfig,
  onGenerateSummary,
  onStopGeneration,
  customPrompt,
  summaryStatus,
  availableTemplates,
  selectedTemplate,
  onTemplateSelect,
  hasTranscripts = true,
  hasSummary = false,
  isModelConfigLoading = false,
  onOpenModelSettings,
  languageSlot, actionsSlot, compact = true, onActionsCloseAutoFocus
}: SummaryGeneratorButtonGroupProps) {
  const [isCheckingModels, setIsCheckingModels] = useState(false);
  const [settingsDialogOpen, setSettingsDialogOpen] = useState(false);

  // Expose the function to open the modal via callback registration
  useEffect(() => {
    if (onOpenModelSettings) {
      // Register our open dialog function with the parent by calling the callback
      // This allows the parent to store a reference to this function
      const openDialog = () => {
        console.log('📱 Opening model settings dialog via callback');
        setSettingsDialogOpen(true);
      };

      // Call the parent's callback with our open function
      // Note: This assumes onOpenModelSettings accepts a function parameter
      // We'll need to adjust the signature
      onOpenModelSettings(openDialog);
    }
  }, [onOpenModelSettings]);


  const checkBuiltInAIModelsAndGenerate = async () => {
    setIsCheckingModels(true);
    try {
      const selectedModel = modelConfig.model;

      // Check if specific model is configured
      if (!selectedModel) {
        toast.error('No built-in AI model selected', {
          description: 'Please select a model in settings',
          duration: 5000,
        });
        setSettingsDialogOpen(true);
        return;
      }

      // Check model readiness (with filesystem refresh)
      const isReady = await invoke<boolean>('builtin_ai_is_model_ready', {
        modelName: selectedModel,
        refresh: true,
      });

      if (isReady) {
        // Model is available, proceed with generation
        onGenerateSummary(customPrompt);
        return;
      }

      // Model not ready - check detailed status
      const modelInfo = await invoke<BuiltInModelInfo | null>('builtin_ai_get_model_info', {
        modelName: selectedModel,
      });

      if (!modelInfo) {
        toast.error('Model not found', {
          description: `Could not find information for model: ${selectedModel}`,
          duration: 5000,
        });
        setSettingsDialogOpen(true);
        return;
      }

      // Handle different model states
      const status = modelInfo.status;

      if (status.type === 'downloading') {
        toast.info('Model download in progress', {
          description: `${selectedModel} is downloading (${status.progress}%). Please wait until download completes.`,
          duration: 5000,
        });
        return;
      }

      if (status.type === 'not_downloaded') {
        toast.error('Model not downloaded', {
          description: `${selectedModel} needs to be downloaded before use. Opening model settings...`,
          duration: 5000,
        });
        setSettingsDialogOpen(true);
        return;
      }

      if (status.type === 'corrupted') {
        toast.error('Model file corrupted', {
          description: `${selectedModel} file is corrupted. Please delete and re-download.`,
          duration: 7000,
        });
        setSettingsDialogOpen(true);
        return;
      }

      if (status.type === 'error') {
        toast.error('Model error', {
          description: status.Error || 'An error occurred with the model',
          duration: 5000,
        });
        setSettingsDialogOpen(true);
        return;
      }

      // Fallback
      toast.error('Model not available', {
        description: 'The selected model is not ready for use',
        duration: 5000,
      });
      setSettingsDialogOpen(true);

    } catch (error) {
      console.error('Error checking built-in AI models:', error);
      toast.error('Failed to check model status', {
        description: error instanceof Error ? error.message : String(error),
        duration: 5000,
      });
    } finally {
      setIsCheckingModels(false);
    }
  };

  const checkOllamaModelsAndGenerate = async () => {
    // Handle built-in AI provider
    if (modelConfig.provider === 'builtin-ai') {
      await checkBuiltInAIModelsAndGenerate();
      return;
    }

    // Only check for Ollama provider
    if (modelConfig.provider !== 'ollama') {
      onGenerateSummary(customPrompt);
      return;
    }

    setIsCheckingModels(true);
    try {
      const endpoint = modelConfig.ollamaEndpoint || null;
      const models = await invoke('get_ollama_models', { endpoint }) as any[];

      if (!models || models.length === 0) {
        // No models available, show message and open settings
        toast.error(
          'No Ollama models found. Please download gemma2:2b from Model settings.',
          { duration: 5000 }
        );
        setSettingsDialogOpen(true);
        return;
      }

      // Models are available, proceed with generation
      onGenerateSummary(customPrompt);
    } catch (error) {
      console.error('Error checking Ollama models:', error);
      const errorMessage = error instanceof Error ? error.message : String(error);

      if (isOllamaNotInstalledError(errorMessage)) {
        // Ollama is not installed - show specific message with download link
        toast.error(
          'Ollama is not installed',
          {
            description: 'Please download and install Ollama to use local models.',
            duration: 7000,
            action: {
              label: 'Download',
              onClick: () => openExternal('https://ollama.com/download')
            }
          }
        );
      } else {
        // Other error - generic message
        toast.error(
          'Failed to check Ollama models. Please check if Ollama is running and download a model.',
          { duration: 5000 }
        );
      }
      setSettingsDialogOpen(true);
    } finally {
      setIsCheckingModels(false);
    }
  };

  const isGenerating = summaryStatus === 'processing' || summaryStatus === 'summarizing' || summaryStatus === 'regenerating';

  const templateItems = availableTemplates.map(template => <DropdownMenuItem key={template.id} onSelect={() => onTemplateSelect(template.id, template.name)} title={template.description} className="flex items-center justify-between gap-2">
    {template.name}{selectedTemplate === template.id && <Check className="h-4 w-4 text-primary" />}
  </DropdownMenuItem>);
  return <div className="flex shrink-0 items-center gap-1">
    {languageSlot}
    {!compact && availableTemplates.length > 0 && <DropdownMenu><DropdownMenuTrigger asChild><Button variant="ghost" size="sm" className="px-2">Template</Button></DropdownMenuTrigger><DropdownMenuContent align="end">{templateItems}</DropdownMenuContent></DropdownMenu>}
    {!compact && <Button variant="ghost" size="sm" className="px-2" onClick={() => setSettingsDialogOpen(true)}>AI model</Button>}
    {isGenerating ? <Button variant="outline" size="sm" onClick={() => { Analytics.trackButtonClick('stop_summary_generation', 'meeting_details'); onStopGeneration(); }}><Square className="h-4 w-4" />Stop</Button>
      : <Button size="sm" className="px-2 text-xs" disabled={!hasTranscripts || isCheckingModels || isModelConfigLoading} onClick={() => { Analytics.trackButtonClick('generate_summary', 'meeting_details'); void checkOllamaModelsAndGenerate(); }}>
        {isCheckingModels || isModelConfigLoading ? <Loader2 className="h-4 w-4 animate-spin" /> : <Sparkles className="h-4 w-4" />}
        {isCheckingModels || isModelConfigLoading ? 'Checking models…' : hasSummary ? 'Regenerate summary' : 'Generate summary'}
      </Button>}
    <DropdownMenu><DropdownMenuTrigger asChild><ActionIcon label="More summary actions" className="h-8 w-8"><MoreHorizontal /></ActionIcon></DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-60" onCloseAutoFocus={onActionsCloseAutoFocus}>
        {compact && availableTemplates.length > 0 && <DropdownMenuSub><DropdownMenuSubTrigger ><FileText className="mr-2 h-4 w-4" />Template</DropdownMenuSubTrigger><DropdownMenuSubContent>{templateItems}</DropdownMenuSubContent></DropdownMenuSub>}
        {compact && <DropdownMenuItem onSelect={() => setSettingsDialogOpen(true)}><Settings className="mr-2 h-4 w-4" />AI model</DropdownMenuItem>}
        {compact && <DropdownMenuSeparator />}
        {actionsSlot}
      </DropdownMenuContent>
    </DropdownMenu>
    <Dialog open={settingsDialogOpen} onOpenChange={setSettingsDialogOpen}>
      <DialogContent aria-describedby={undefined}><VisuallyHidden><DialogTitle>Model settings</DialogTitle></VisuallyHidden>
        <ModelSettingsModal onSave={async config => { await onSaveModelConfig(config); setSettingsDialogOpen(false); }} modelConfig={modelConfig} setModelConfig={setModelConfig} skipInitialFetch={true} layout="dialog" />
      </DialogContent>
    </Dialog>
  </div>;
}
