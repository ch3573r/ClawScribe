import { isActiveSummary, type SummaryJobSnapshot } from '@/lib/summary-job-status';
import { confirm } from '@tauri-apps/plugin-dialog';
import { confirmSummaryRegeneration } from '@/lib/meetingDrafts';
import { openExternal } from '@/lib/openExternal';
import { useState, useCallback, useEffect, useRef } from 'react';
import type { MeetingMetadata, MeetingSummary } from '@/types';
import type { OllamaModel } from '@/contexts/ConfigContext';
import { isEditorSummary, readMeetingSummary, summaryDetails } from '@/lib/meeting-summary';
import { ModelConfig } from '@/components/ModelSettingsModal';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { invoke as invokeTauri } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import Analytics from '@/lib/analytics';
import { isOllamaNotInstalledError } from '@/lib/utils';
import { BuiltInModelInfo } from '@/lib/builtin-ai';
import { getMeetingContext } from '@/lib/meetingContext';
import { readMeetingSummaryLanguage } from '@/lib/summary-language-preferences';

type SummaryStatus = 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error';

async function checkSummaryModel(config: ModelConfig, openSettings?: () => void): Promise<boolean> {
  if (config.provider === 'ollama') {
    try {
      const models = await invokeTauri<OllamaModel[]>('get_ollama_models', { endpoint: config.ollamaEndpoint || null });
      if (models?.length) return true;
      toast.error('No Ollama models found. Please download gemma3:1b from Model Settings.', { duration: 5000 });
    } catch (error) {
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
    if (await invokeTauri<boolean>('builtin_ai_is_model_ready', { modelName, refresh: true })) return true;
    const model = await invokeTauri<BuiltInModelInfo | null>('builtin_ai_get_model_info', { modelName });
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
    toast.error('Failed to validate built-in AI model', { description: error instanceof Error ? error.message : String(error), duration: 5000 });
  }
  return false;
}

interface UseSummaryGenerationProps {
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

  const { startSummaryPolling, attachSummaryPolling } = useSidebar();
  const [jobUpdate, setJobUpdate] = useState<SummaryJobSnapshot | null>(null);
  const starting = useRef(false);
  // Lets go of a run this page attached to when the page goes away. A background
  // poll (the automatic summary) then carries on and finishes on its own.
  const detachSummary = useRef<(() => void) | null>(null);
  useEffect(() => () => { detachSummary.current?.(); detachSummary.current = null; }, []);

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

  // What to do with each poll result: restore, error or completion. Shared by a
  // run this page started and a run it attached to.
  const handleSummaryUpdate = useCallback(async (pollingResult: SummaryJobSnapshot, isRegeneration: boolean) => {
    setJobUpdate(pollingResult);
    if (pollingResult.status === 'skipped') {
      setSummaryStatus('idle');
      setSummaryError(null);
      return;
    }

    // Handle cancellation
    if (pollingResult.status === 'cancelled') {
      console.log('Summary generation was cancelled');

      // Reload summary from database (backend has already restored from backup)
      try {
        const existingSummary = await invokeTauri<SummaryJobSnapshot>('api_get_summary', {
          meetingId: meeting.id
        });

        const restored = readMeetingSummary(existingSummary?.data).summary;
        if (restored) {
          console.log('Restored previous summary after cancellation');
          setAiSummary(restored);
          setSummaryStatus('completed');
        } else {
          setSummaryStatus('idle');
        }
      } catch (error) {
        console.error('Failed to reload summary after cancellation:', error);
        setSummaryStatus('idle');
      }

      setSummaryError(null);
      return;
    }

    // Handle errors
    if (pollingResult.status === 'error' || pollingResult.status === 'failed') {
      console.error('Backend returned error:', pollingResult.error);
      const errorMessage = pollingResult.error || `Summary ${isRegeneration ? 'regeneration' : 'generation'} failed`;

      // If this was a regeneration, try to restore previous summary from database
      if (isRegeneration) {
        try {
          const existingSummary = await invokeTauri<SummaryJobSnapshot>('api_get_summary', {
            meetingId: meeting.id
          });

          const restored = readMeetingSummary(existingSummary?.data).summary;
          if (restored) {
            console.log('Restored previous summary after regeneration failure');
            setAiSummary(restored);
            setSummaryStatus('completed');
            setSummaryError(null);

            // Show error toast with restoration message
            toast.error(`Failed to regenerate summary`, {
              description: `${errorMessage}. Your previous summary has been restored.`,
            });

            await Analytics.trackSummaryGenerationCompleted(
              modelConfig.provider,
              modelConfig.model,
              false,
              undefined,
              errorMessage
            );
            return;
          }
        } catch (error) {
          console.error('Failed to reload summary after error:', error);
        }
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

      await Analytics.trackSummaryGenerationCompleted(
        modelConfig.provider,
        modelConfig.model,
        false,
        undefined,
        errorMessage
      );
      return;
    }

    // Handle successful completion
    if (pollingResult.status === 'completed' && pollingResult.data) {
      console.log('Summary generation completed');

      // Update meeting title if available
      const result = readMeetingSummary(pollingResult.data);
      const meetingName = result.meetingName || pollingResult.meetingName;
      if (meetingName) {
        updateMeetingTitle(meetingName);
      }

      const summary = result.summary;
      if (!summary || (!isEditorSummary(summary) && !Object.values(summary).some(section => section.blocks.length))) {
        console.error('Summary completed but all sections empty');
        setSummaryError('Summary generation completed but returned empty content.');
        setSummaryStatus('error');

        await Analytics.trackSummaryGenerationCompleted(
          modelConfig.provider,
          modelConfig.model,
          false,
          undefined,
          'Empty summary generated'
        );
        return;
      }

      setAiSummary(summary);
      setSummaryStatus('completed');

      // Show success toast
      if (summaryDetails(pollingResult.data).translation) {
        toast.warning('English notes saved. Translation needs a retry.', { id: `summary-ready:${meeting.id}` });
      } else {
        toast.success('Summary ready', { id: `summary-ready:${meeting.id}`, description: meetingName || meeting.title, duration: 4000 });
      }

      await Analytics.trackSummaryGenerationCompleted(
        modelConfig.provider,
        modelConfig.model,
        true
      );

      if (meetingName && onMeetingUpdated) {
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
        message => confirm(message, { title: 'Regenerate summary', kind: 'warning' }),
      );
      if (!accepted) return;
    } catch {
      toast.error('Save your pending edits before generating a summary.');
      return;
    }
    setSummaryStatus(isRegeneration ? 'regenerating' : 'processing');
    setSummaryError(null);
    setJobUpdate(null);

    try {
      console.log('Processing transcript with template:', selectedTemplate);

      // Calculate time since recording
      const timeSinceRecording = (Date.now() - new Date(meeting.created_at).getTime()) / 60000; // minutes

      // Track summary generation started
      await Analytics.trackSummaryGenerationStarted(
        modelConfig.provider,
        modelConfig.model,
        undefined,
        timeSinceRecording
      );

      // Track custom prompt usage if present
      if (customPrompt.trim().length > 0) {
        await Analytics.trackCustomPromptUsed(customPrompt.trim().length);
      }

      // Show toast notification for generation start
      toast.info(`${isRegeneration ? 'Regenerating' : 'Generating'} summary...`, {
        description: `Using ${modelConfig.provider}/${modelConfig.model}`,
        duration: 3000,
      });

      const summaryLanguage = translationTarget ?? await readMeetingSummaryLanguage(meeting.id)
        .then(preference => preference.language)
        .catch(() => null);

      const result = await invokeTauri<{ process_id: string }>('api_generate_meeting_summary', {
        model: modelConfig.provider,
        modelName: modelConfig.model,
        meetingId: meeting.id,
        customPrompt: customPrompt,
        templateId: selectedTemplate || null,
        summaryLanguage,
        includeSpeakerLabels,
        ...(translationTarget ? { retryTranslation: true } : {}),
      });

      const process_id = result.process_id;
      console.log('Process ID:', process_id);

      // Start global polling via context
      // The poll reports to this handler; a later visit to the page can attach to
      // the same run (see attachToRunningSummary).
      startSummaryPolling(meeting.id, process_id, pollingResult => handleSummaryUpdate(pollingResult, isRegeneration));
    } catch (error) {
      const errorMessage = typeof error === 'string' ? error : error instanceof Error ? error.message : 'Could not start summary generation';
      if (errorMessage.includes('already has a summary in progress')) {
        detachSummary.current?.();
        detachSummary.current = attachSummaryPolling(meeting.id, result => handleSummaryUpdate(result, isRegeneration));
        return;
      }
      console.error(`Failed to ${isRegeneration ? 'regenerate' : 'generate'} summary:`, error);
      setSummaryError(errorMessage);
      setSummaryStatus('error');
      // Note: We don't clear the summary here because the backend has already restored from backup

      toast.error(`Failed to ${isRegeneration ? 'regenerate' : 'generate'} summary`, {
        description: errorMessage,
      });

      await Analytics.trackSummaryGenerationCompleted(
        modelConfig.provider,
        modelConfig.model,
        false,
        undefined,
        errorMessage
      );
    }
  }, [
    attachSummaryPolling,
    beforeGeneration,
    meeting.id,
    meeting.created_at,
    modelConfig,
    selectedTemplate,
    includeSpeakerLabels,
    startSummaryPolling,
    handleSummaryUpdate,
  ]);

  const processSummary = useCallback(async (request: Parameters<typeof startSummary>[0]) => {
    if (starting.current || isActiveSummary(summaryStatus)) return;
    if (isModelConfigLoading) {
      toast.info('Loading model configuration, please wait...');
      return;
    }
    starting.current = true;
    try {
      let customPrompt = request.customPrompt ?? '';
      if (!request.translationTarget && !customPrompt.trim()) {
        try {
          customPrompt = await getMeetingContext(meeting.id);
        } catch {
          toast.error('Could not load meeting context. Try generating again.');
          return;
        }
      }
      if (modelConfig.provider === 'ollama' || modelConfig.provider === 'builtin-ai') {
        setIsCheckingModels(true);
        try {
          if (!await checkSummaryModel(modelConfig, onOpenModelSettings)) return;
        } finally {
          setIsCheckingModels(false);
        }
      }
      await startSummary({ ...request, customPrompt });
    } finally {
      starting.current = false;
    }
  }, [startSummary, summaryStatus, isModelConfigLoading, modelConfig, meeting.id, onOpenModelSettings]);

  const handleGenerateSummary = useCallback((customPrompt = '') =>
    processSummary({ customPrompt, isRegeneration: hasExistingSummary }), [processSummary, hasExistingSummary]);

  const handleRegenerateSummary = useCallback((customPrompt = '') =>
    processSummary({ customPrompt, isRegeneration: true }), [processSummary]);

  // A summary for this meeting is already running in the core (started by the
  // automatic summary after a recording, or by this page before the user left and
  // came back). Show it as generating and take its result when it finishes.
  const attachToRunningSummary = useCallback((hasExistingSummary = false) => {
    const isRegeneration = hasExistingSummary;
    setSummaryStatus(isRegeneration ? 'regenerating' : 'processing');
    setSummaryError(null);
    setJobUpdate(null);
    detachSummary.current?.();
    detachSummary.current = attachSummaryPolling(meeting.id, pollingResult => handleSummaryUpdate(pollingResult, isRegeneration));
  }, [meeting.id, attachSummaryPolling, handleSummaryUpdate]);

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
