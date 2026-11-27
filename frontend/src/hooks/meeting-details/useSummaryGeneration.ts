import { isActiveSummary, isTerminalSummary, type SummaryJobSnapshot } from '@/lib/summary-job-status';
import type { SubscribeToSummary } from '@/lib/summary-updates';
import { confirm } from '@tauri-apps/plugin-dialog';
import { confirmSummaryRegeneration } from '@/lib/meetingDrafts';
import { openExternal } from '@/lib/openExternal';
import { useState, useCallback, useEffect, useRef } from 'react';
import type { MeetingMetadata, MeetingSummary } from '@/types';
import type { OllamaModel } from '@/contexts/ConfigContext';
import { isEditorSummary, readMeetingSummary, summaryDetails } from '@/lib/meeting-summary';
import { ModelConfig } from '@/components/ModelSettingsModal';
import { invoke as invokeTauri } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { isOllamaNotInstalledError } from '@/lib/utils';
import { BuiltInModelInfo } from '@/lib/builtin-ai';
import { getMeetingContext } from '@/lib/meetingContext';
import { readMeetingSummaryLanguage } from '@/lib/summary-language-preferences';

type SummaryStatus = 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error';

async function checkSummaryModel(config: ModelConfig, isMounted: () => boolean, openSettings?: () => void): Promise<boolean> {
  if (config.provider === 'ollama') {
    try {
      const models = await invokeTauri<OllamaModel[]>('get_ollama_models', { endpoint: config.ollamaEndpoint || null });
      if (!isMounted()) return false;
      if (models?.length) return true;
      toast.error('No Ollama models found. Please download gemma3:1b from Model Settings.', { duration: 5000 });
    } catch (error) {
      if (!isMounted()) return false;
      const message = error instanceof Error ? error.message : String(error);
      if (isOllamaNotInstalledError(message)) {
        toast.error('Ollama is not installed', {
          description: 'Please download and install Ollama to use local models.',
          duration: 7000,
          action: { label: 'Download', onClick: () => openExternal('https://ollama.com/download') },
        });
      } else {
        toast.error('Failed to check Ollama models. Please ensure Ollama is running and download a model from Settings.', { duration: 5000 });
      }
    }
    openSettings?.();
    return false;
  }

  const modelName = config.model;
  if (!modelName) {
    toast.error('No built-in AI model selected', { description: 'Please select a model in settings', duration: 5000 });
    openSettings?.();
    return false;
  }
  try {
    const ready = await invokeTauri<boolean>('builtin_ai_is_model_ready', { modelName, refresh: true });
    if (!isMounted()) return false;
    if (ready) return true;
    const model = await invokeTauri<BuiltInModelInfo | null>('builtin_ai_get_model_info', { modelName });
    if (!isMounted()) return false;
    const status = model?.status;
    if (status?.type === 'downloading') {
      toast.info('Model download in progress', {
        description: `${modelName} is downloading (${status.progress}%). Please wait until download completes.`,
        duration: 5000,
      });
      return false;
    }
    if (status?.type === 'not_downloaded') {
      toast.error('Built-in AI model not downloaded', {
        description: `${modelName} needs to be downloaded. Please download it in model settings.`, duration: 7000,
      });
    } else if (status?.type === 'corrupted' || status?.type === 'error') {
      const description = status.type === 'error' ? status.Error || 'The model file has an error' : 'The model file is corrupted';
      toast.error('Built-in AI model not available', { description: `${description}. Please check model settings.`, duration: 7000 });
    } else {
      toast.error('Built-in AI model not ready', { description: 'Please ensure the model is downloaded in settings', duration: 5000 });
    }
    openSettings?.();
  } catch (error) {
    if (!isMounted()) return false;
    toast.error('Failed to validate built-in AI model', { description: error instanceof Error ? error.message : String(error), duration: 5000 });
  }
  return false;
}

interface UseSummaryGenerationProps {
  subscribeToSummary: SubscribeToSummary;
  beforeGeneration?: () => Promise<void>;
  hasExistingSummary?: boolean;
  meeting: MeetingMetadata;
  modelConfig: ModelConfig;
  isModelConfigLoading: boolean;
  selectedTemplate: string;
  includeSpeakerLabels?: boolean;
  onMeetingUpdated?: () => Promise<void>;
  updateMeetingTitle: (title: string) => void;
  setAiSummary: (summary: MeetingSummary | null) => void;
  onOpenModelSettings?: () => void;
}

export function useSummaryGeneration({
  subscribeToSummary,
  beforeGeneration,
  hasExistingSummary = false,
  meeting,
  modelConfig,
  isModelConfigLoading,
  selectedTemplate,
  includeSpeakerLabels = true,
  onMeetingUpdated,
  updateMeetingTitle,
  setAiSummary,
  onOpenModelSettings,
}: UseSummaryGenerationProps) {
  const [summaryStatus, setSummaryStatus] = useState<SummaryStatus>('idle');
  const [summaryError, setSummaryError] = useState<string | null>(null);
  const [isCheckingModels, setIsCheckingModels] = useState(false);

  const [jobUpdate, setJobUpdate] = useState<SummaryJobSnapshot | null>(null);
  const starting = useRef(false);
  const mounted = useRef(false);
  // Navigation releases this view's subscription. Native generation continues.
  const detachSummary = useRef<(() => void) | null>(null);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      detachSummary.current?.();
      detachSummary.current = null;
    };
  }, []);

  // Helper to get status message
  const getSummaryStatusMessage = useCallback((status: SummaryStatus) => {
    switch (status) {
      case 'processing':
        return 'Processing transcript...';
      case 'summarizing':
        return 'Generating summary...';
      case 'regenerating':
        return 'Regenerating summary...';
      case 'completed':
        return 'Summary completed';
      case 'error':
        return 'Error generating summary';
      default:
        return '';
    }
  }, []);

  // Apply restore, error or completion to the current view. Shared by a
  // run this page started and a run it attached to.
  const handleSummaryUpdate = useCallback(async (snapshot: SummaryJobSnapshot, isRegeneration: boolean) => {
    if (!mounted.current) return;
    setJobUpdate(snapshot);
    if (snapshot.status === 'skipped') {
      setSummaryStatus('idle');
      setSummaryError(null);
      return;
    }

    // Handle cancellation
    if (snapshot.status === 'cancelled') {
      console.log('Summary generation was cancelled');

      const restored = readMeetingSummary(snapshot.data).summary;
      setAiSummary(restored);
      setSummaryStatus(restored ? 'completed' : 'idle');

      setSummaryError(null);
      return;
    }

    // Handle errors
    if (snapshot.status === 'error' || snapshot.status === 'failed') {
      console.error('Backend returned error:', snapshot.error);
      const errorMessage = snapshot.error || `Summary ${isRegeneration ? 'regeneration' : 'generation'} failed`;

      const restored = isRegeneration ? readMeetingSummary(snapshot.data).summary : null;
      if (restored) {
        setAiSummary(restored);
        setSummaryStatus('completed');
        setSummaryError(null);
        toast.error('Failed to regenerate summary', {
          description: `${errorMessage}. Your previous summary has been restored.`,
        });
        return;
      }

      // Continue with normal error handling if not regeneration or reload failed
      setSummaryError(errorMessage);
      setSummaryStatus('error');

      // Check if this is a "model is required" error
      const isModelRequiredError = errorMessage.includes('model is required') ||
        errorMessage.includes('"model":"required"') ||
        errorMessage.toLowerCase().includes('model') && errorMessage.toLowerCase().includes('required');

      // Show error toast
      toast.error(`Failed to ${isRegeneration ? 'regenerate' : 'generate'} summary`, {
        description: errorMessage.includes('Connection refused')
          ? 'Could not connect to LLM service. Please ensure Ollama or your configured LLM provider is running.'
          : errorMessage,
      });

      // Auto-open model settings modal if model is missing
      if (isModelRequiredError && onOpenModelSettings) {
        console.log('🔧 Model required error detected, opening model settings...');
        onOpenModelSettings();
      }

      return;
    }

    // Handle successful completion
    if (snapshot.status === 'completed' && snapshot.data) {
      console.log('Summary generation completed');

      // Update meeting title if available
      const result = readMeetingSummary(snapshot.data);
      const meetingName = result.meetingName || snapshot.meetingName;
      if (meetingName) {
        updateMeetingTitle(meetingName);
      }

      const summary = result.summary;
      if (!summary || (!isEditorSummary(summary) && !Object.values(summary).some(section => section.blocks.length))) {
        console.error('Summary completed but all sections empty');
        setSummaryError('Summary generation completed but returned empty content.');
        setSummaryStatus('error');

        return;
      }

      setAiSummary(summary);
      setSummaryStatus('completed');

      // Show success toast
      if (summaryDetails(snapshot.data).translation) {
        toast.warning('English notes saved. Translation needs a retry.', { id: `summary-ready:${meeting.id}` });
      } else {
        toast.success('Summary ready', { id: `summary-ready:${meeting.id}`, description: meetingName || meeting.title, duration: 4000 });
      }


      if (mounted.current && meetingName && onMeetingUpdated) {
        await onMeetingUpdated();
      }
    }
  }, [
    meeting.id,
    meeting.title,
    modelConfig,
    setAiSummary,
    updateMeetingTitle,
    onMeetingUpdated,
    onOpenModelSettings,
  ]);

  const watchSummary = useCallback((isRegeneration: boolean, newRun = false) => {
    if (!mounted.current) return;
    detachSummary.current?.();
    detachSummary.current = subscribeToSummary(result => {
      if (isTerminalSummary(result.status)) {
        detachSummary.current?.();
        detachSummary.current = null;
      }
      if (result.status === 'idle') {
        result = { status: 'error', error: 'Summary generation is no longer active. Please generate the summary again.' };
      } else if (result.status === 'completed' && !result.data) {
        result = { status: 'error', error: 'Summary generation completed without saved content. Please try again.' };
      }
      return handleSummaryUpdate(result, isRegeneration);
    }, { refresh: newRun });
  }, [subscribeToSummary, handleSummaryUpdate]);

  // Unified summary processing logic
  const startSummary = useCallback(async ({
    customPrompt = '',
    isRegeneration = false,
    translationTarget,
  }: {
    customPrompt?: string;
    isRegeneration?: boolean;
    translationTarget?: string;
  }) => {
    try {
      const accepted = await confirmSummaryRegeneration(
        beforeGeneration ?? (async () => {}),
        () => invokeTauri<{ data?: { user_edited_at?: unknown } }>('api_get_summary', { meetingId: meeting.id }),
        message => mounted.current ? confirm(message, { title: 'Regenerate summary', kind: 'warning' }) : Promise.resolve(false),
      );
      if (!mounted.current || !accepted) return;
    } catch {
      if (mounted.current) toast.error('Save your pending edits before generating a summary.');
      return;
    }
    if (!translationTarget && !customPrompt.trim()) {
      try {
        customPrompt = await getMeetingContext(meeting.id);
      } catch {
        if (mounted.current) toast.error('Could not load meeting context. Try generating again.');
        return;
      }
      if (!mounted.current) return;
    }
    setSummaryStatus(isRegeneration ? 'regenerating' : 'processing');
    setSummaryError(null);
    setJobUpdate(null);

    try {
      console.log('Processing transcript with template:', selectedTemplate);

      if (!mounted.current) return;

      // Show toast notification for generation start
      toast.info(`${isRegeneration ? 'Regenerating' : 'Generating'} summary...`, {
        description: `Using ${modelConfig.provider}/${modelConfig.model}`,
        duration: 3000,
      });

      const summaryLanguage = translationTarget ?? await readMeetingSummaryLanguage(meeting.id)
        .then(preference => preference.language)
        .catch(() => null);
      if (!mounted.current) return;

      await invokeTauri('api_generate_meeting_summary', {
        model: modelConfig.provider,
        modelName: modelConfig.model,
        meetingId: meeting.id,
        customPrompt: customPrompt,
        templateId: selectedTemplate || null,
        summaryLanguage,
        includeSpeakerLabels,
        ...(translationTarget ? { retryTranslation: true } : {}),
      });

      watchSummary(isRegeneration, true);
    } catch (error) {
      if (!mounted.current) return;
      const errorMessage = typeof error === 'string' ? error : error instanceof Error ? error.message : 'Could not start summary generation';
      if (errorMessage.includes('already has a summary in progress')) {
        watchSummary(isRegeneration);
        return;
      }
      console.error(`Failed to ${isRegeneration ? 'regenerate' : 'generate'} summary:`, error);
      setSummaryError(errorMessage);
      setSummaryStatus('error');
      // Note: We don't clear the summary here because the backend has already restored from backup

      toast.error(`Failed to ${isRegeneration ? 'regenerate' : 'generate'} summary`, {
        description: errorMessage,
      });

    }
  }, [
    beforeGeneration,
    meeting.id,
    meeting.created_at,
    modelConfig,
    selectedTemplate,
    includeSpeakerLabels,
    watchSummary,
  ]);

  const processSummary = useCallback(async (request: Parameters<typeof startSummary>[0]) => {
    if (!mounted.current || starting.current || isActiveSummary(summaryStatus)) return;
    if (isModelConfigLoading) {
      toast.info('Loading model configuration, please wait...');
      return;
    }
    starting.current = true;
    try {
      if (modelConfig.provider === 'ollama' || modelConfig.provider === 'builtin-ai') {
        setIsCheckingModels(true);
        try {
          if (!await checkSummaryModel(modelConfig, () => mounted.current, onOpenModelSettings)) return;
        } finally {
          if (mounted.current) setIsCheckingModels(false);
        }
      }
      if (mounted.current) await startSummary(request);
    } finally {
      starting.current = false;
    }
  }, [startSummary, summaryStatus, isModelConfigLoading, modelConfig, onOpenModelSettings]);

  const handleGenerateSummary = useCallback((customPrompt = '') =>
    processSummary({ customPrompt, isRegeneration: hasExistingSummary }), [processSummary, hasExistingSummary]);

  const handleRegenerateSummary = useCallback((customPrompt = '') =>
    processSummary({ customPrompt, isRegeneration: true }), [processSummary]);

  // A summary for this meeting is already running in the core (started by the
  // automatic summary after a recording, or by this page before the user left and
  // came back). Show it as generating and take its result when it finishes.
  const attachToRunningSummary = useCallback((hasExistingSummary = false) => {
    if (!mounted.current) return;
    const isRegeneration = hasExistingSummary;
    setSummaryStatus(isRegeneration ? 'regenerating' : 'processing');
    setSummaryError(null);
    setJobUpdate(null);
    watchSummary(isRegeneration);
  }, [watchSummary]);

  const handleRetryTranslation = useCallback(async (target: string, customPrompt: string) => {
    await processSummary({ customPrompt, isRegeneration: true, translationTarget: target });
  }, [processSummary]);

  const handleStopGeneration = useCallback(async () => {
    try {
      await invokeTauri('api_cancel_summary', { meetingId: meeting.id });
    } catch {
      toast.error('Could not stop summary generation');
    }
  }, [meeting.id]);

  return {
    summaryStatus,
    summaryError,
    isCheckingModels,
    jobUpdate,
    handleGenerateSummary,
    handleRegenerateSummary,
    handleRetryTranslation,
    handleStopGeneration,
    attachToRunningSummary,
    getSummaryStatusMessage,
  };
}
