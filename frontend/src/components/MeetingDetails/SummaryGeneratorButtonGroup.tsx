"use client";

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
import { useState, useEffect, ReactNode } from 'react';

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
  showRegenerate?: boolean;
  generationDisabled?: boolean;
  isCheckingModels?: boolean;
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
  showRegenerate = false,
  generationDisabled = false,
  isCheckingModels = false,
  onOpenModelSettings,
  languageSlot, actionsSlot, compact = true, onActionsCloseAutoFocus,
}: SummaryGeneratorButtonGroupProps) {
  const [settingsDialogOpen, setSettingsDialogOpen] = useState(false);

  // Preflight failures can open the same settings dialog as the toolbar.
  useEffect(() => {
    onOpenModelSettings?.(() => setSettingsDialogOpen(true));
  }, [onOpenModelSettings]);

  const isGenerating = summaryStatus === 'processing' || summaryStatus === 'summarizing' || summaryStatus === 'regenerating';

  const templateItems = availableTemplates.map(template => <DropdownMenuItem key={template.id} onSelect={() => onTemplateSelect(template.id, template.name)} title={template.description} className="flex items-center justify-between gap-2">
    {template.name}{selectedTemplate === template.id && <Check className="h-4 w-4 text-primary" />}
  </DropdownMenuItem>);
  return <div className="flex shrink-0 items-center gap-1">
    {languageSlot}
    {!compact && availableTemplates.length > 0 && <DropdownMenu><DropdownMenuTrigger asChild><Button variant="ghost" size="sm" className="px-2">Template</Button></DropdownMenuTrigger><DropdownMenuContent align="end">{templateItems}</DropdownMenuContent></DropdownMenu>}
    {!compact && <Button variant="ghost" size="sm" className="px-2" onClick={() => setSettingsDialogOpen(true)}>AI model</Button>}
    {isGenerating ? <Button variant="outline" size="sm" onClick={() => { Analytics.trackButtonClick('stop_summary_generation', 'meeting_details'); onStopGeneration(); }}><Square className="h-4 w-4" />Stop</Button>
      : showRegenerate && <Button size="sm" className="px-2 text-xs" disabled={generationDisabled || isCheckingModels} onClick={() => { Analytics.trackButtonClick('generate_summary', 'meeting_details'); void onGenerateSummary(customPrompt); }}>
        {isCheckingModels ? <Loader2 className="h-4 w-4 animate-spin motion-reduce:animate-none" /> : <Sparkles className="h-4 w-4" />}
        {isCheckingModels ? 'Checking models…' : 'Regenerate summary'}
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
