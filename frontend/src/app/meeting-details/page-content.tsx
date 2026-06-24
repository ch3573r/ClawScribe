"use client";
import { useState, useEffect, useRef, useCallback } from 'react';
import { getMeetingContext, setMeetingContext } from '@/lib/meetingContext';
import { motion } from 'framer-motion';
import { Summary, SummaryResponse } from '@/types';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import Analytics from '@/lib/analytics';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { TranscriptPanel } from '@/components/MeetingDetails/TranscriptPanel';
import { SummaryPanel } from '@/components/MeetingDetails/SummaryPanel';
import { MeetingChat } from '@/components/MeetingDetails/MeetingChat';
import { SpeakerLaneTimeline } from '@/components/MeetingDetails/SpeakerLaneTimeline';
import { ModelConfig } from '@/components/ModelSettingsModal';

// Custom hooks
import { useMeetingData } from '@/hooks/meeting-details/useMeetingData';
import { useSummaryGeneration } from '@/hooks/meeting-details/useSummaryGeneration';
import { useTemplates } from '@/hooks/meeting-details/useTemplates';
import { useCopyOperations } from '@/hooks/meeting-details/useCopyOperations';
import { useMeetingOperations } from '@/hooks/meeting-details/useMeetingOperations';
import { useConfig } from '@/contexts/ConfigContext';
import { useSourceAttribution } from '@/hooks/useSourceAttribution';
import { useAudioPlayer } from '@/hooks/useAudioPlayer';

export default function PageContent({
  meeting,
  summaryData,
  shouldAutoGenerate = false,
  onAutoGenerateComplete,
  onMeetingUpdated,
  onRefetchTranscripts,
  onUpdateTranscriptSpeaker,
  onApplySpeakerToMatching,
  // Pagination props for efficient transcript loading
  segments,
  hasMore,
  isLoadingMore,
  totalCount,
  loadedCount,
  onLoadMore,
}: {
  meeting: any;
  summaryData: Summary | null;
  shouldAutoGenerate?: boolean;
  onAutoGenerateComplete?: () => void;
  onMeetingUpdated?: () => Promise<void>;
  onRefetchTranscripts?: () => Promise<void>;
  onUpdateTranscriptSpeaker?: (transcriptId: string, speaker: string | null) => Promise<void>;
  onApplySpeakerToMatching?: (fromSpeaker: string | null | undefined, speaker: string | null) => Promise<number>;
  // Pagination props
  segments?: any[];
  hasMore?: boolean;
  isLoadingMore?: boolean;
  totalCount?: number;
  loadedCount?: number;
  onLoadMore?: () => void;
}) {
  console.log('📄 PAGE CONTENT: Initializing with data:', {
    meetingId: meeting.id,
    summaryDataKeys: summaryData ? Object.keys(summaryData) : null,
    transcriptsCount: meeting.transcripts?.length
  });

  // State — "Add context" is persisted per meeting so it survives reopening and
  // is applied on every generate/regenerate.
  const [customPrompt, setCustomPrompt] = useState<string>(() =>
    getMeetingContext(meeting.id),
  );
  // Reload the stored context when switching meetings without a remount.
  useEffect(() => {
    setCustomPrompt(getMeetingContext(meeting.id));
  }, [meeting.id]);
  // Persist edits against the current meeting (only on user change, so switching
  // meetings can't cross-write the previous meeting's text).
  const handlePromptChange = useCallback(
    (value: string) => {
      setCustomPrompt(value);
      setMeetingContext(meeting.id, value);
    },
    [meeting.id],
  );
  const [isRecording] = useState(false);
  const [summaryResponse] = useState<SummaryResponse | null>(null);
  const [audioPath, setAudioPath] = useState<string | null>(null);
  const audioPlayer = useAudioPlayer(audioPath);
  const isAudioReady = Boolean(audioPath && audioPlayer.duration > 0 && !audioPlayer.error);

  // Ref to store the modal open function from SummaryGeneratorButtonGroup
  const openModelSettingsRef = useRef<(() => void) | null>(null);

  // Sidebar context
  const { serverAddress } = useSidebar();

  // Get model config from ConfigContext
  const { modelConfig, setModelConfig } = useConfig();
  const sourceAttributionEnabled = useSourceAttribution();
  const hasSavedAudio = Boolean(audioPath);
  const showSpeakerAttribution = sourceAttributionEnabled && hasSavedAudio;

  // Custom hooks
  const meetingData = useMeetingData({ meeting, summaryData, onMeetingUpdated });
  const templates = useTemplates();

  // Callback to register the modal open function
  const handleRegisterModalOpen = (openFn: () => void) => {
    console.log('📝 Registering modal open function in PageContent');
    openModelSettingsRef.current = openFn;
  };

  // Callback to trigger modal open (called from error handler)
  const handleOpenModelSettings = () => {
    console.log('🔔 Opening model settings from PageContent');
    if (openModelSettingsRef.current) {
      openModelSettingsRef.current();
    } else {
      console.warn('⚠️ Modal open function not yet registered');
    }
  };

  // Save model config to backend database and sync via event
  const handleSaveModelConfig = async (config?: ModelConfig) => {
    if (!config) return;
    try {
      await invoke('api_save_model_config', {
        provider: config.provider,
        model: config.model,
        whisperModel: config.whisperModel,
        apiKey: config.apiKey ?? null,
        ollamaEndpoint: config.ollamaEndpoint ?? null,
      });

      // Emit event so ConfigContext and other listeners stay in sync
      const { emit } = await import('@tauri-apps/api/event');
      await emit('model-config-updated', config);

      toast.success('Model settings saved successfully');
    } catch (error) {
      console.error('Failed to save model config:', error);
      toast.error('Failed to save model settings');
    }
  };

  const summaryGeneration = useSummaryGeneration({
    meeting,
    transcripts: meetingData.transcripts,
    modelConfig: modelConfig,
    isModelConfigLoading: false, // ConfigContext loads on mount
    selectedTemplate: templates.selectedTemplate,
    includeSpeakerLabels: showSpeakerAttribution,
    onMeetingUpdated,
    updateMeetingTitle: meetingData.updateMeetingTitle,
    setAiSummary: meetingData.setAiSummary,
    onOpenModelSettings: handleOpenModelSettings,
  });

  const copyOperations = useCopyOperations({
    meeting,
    transcripts: meetingData.transcripts,
    meetingTitle: meetingData.meetingTitle,
    aiSummary: meetingData.aiSummary,
    blockNoteSummaryRef: meetingData.blockNoteSummaryRef,
    includeSpeakerLabels: showSpeakerAttribution,
  });

  const meetingOperations = useMeetingOperations({
    meeting,
  });

  useEffect(() => {
    let cancelled = false;
    const folderPath = meeting.folder_path;

    if (!folderPath) {
      setAudioPath(null);
      return;
    }

    invoke<string | null>('resolve_meeting_audio_file', { meetingFolder: folderPath })
      .then((path) => {
        if (!cancelled) {
          setAudioPath(path);
        }
      })
      .catch((error) => {
        console.warn('Could not resolve meeting audio file:', error);
        if (!cancelled) {
          setAudioPath(null);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [meeting.folder_path]);

  const handleTimelineSeek = useCallback((seconds: number) => {
    void audioPlayer.seek(seconds);
  }, [audioPlayer]);

  const handleTimelinePlayPause = useCallback(() => {
    if (audioPlayer.isPlaying) {
      audioPlayer.pause();
    } else {
      void audioPlayer.play();
    }
  }, [audioPlayer]);

  // Track page view
  useEffect(() => {
    Analytics.trackPageView('meeting_details');
  }, []);

  // Ctrl/⌘+G — generate / regenerate the summary (dispatched by AppShortcuts).
  useEffect(() => {
    const onGenerate = () => {
      if (meetingData.transcripts.length > 0) {
        void summaryGeneration.handleGenerateSummary('');
      }
    };
    window.addEventListener('shortcut:generate-summary', onGenerate);
    return () => window.removeEventListener('shortcut:generate-summary', onGenerate);
  }, [meetingData.transcripts.length, summaryGeneration]);

  // Auto-generate summary when flag is set
  useEffect(() => {
    let cancelled = false;

    const autoGenerate = async () => {
      if (shouldAutoGenerate && meetingData.transcripts.length > 0 && !cancelled) {
        console.log(`🤖 Auto-generating summary with ${modelConfig.provider}/${modelConfig.model}...`);
        await summaryGeneration.handleGenerateSummary('');

        // Notify parent that auto-generation is complete (only if not cancelled)
        if (onAutoGenerateComplete && !cancelled) {
          onAutoGenerateComplete();
        }
      }
    };

    autoGenerate();

    // Cleanup: cancel if component unmounts or meeting changes
    return () => {
      cancelled = true;
    };
  }, [shouldAutoGenerate, meeting.id]); // Re-run if meeting changes

  return (
    <motion.div
      initial={{ opacity: 0, y: 20 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3, ease: 'easeOut' }}
      className="flex h-full flex-col bg-background"
    >
      {showSpeakerAttribution && (
        <SpeakerLaneTimeline
          segments={segments ?? []}
          totalCount={totalCount}
          loadedCount={loadedCount}
          currentTime={audioPlayer.currentTime}
          durationSeconds={audioPlayer.duration || undefined}
          isPlaying={audioPlayer.isPlaying}
          isAudioReady={isAudioReady}
          onPlayPause={handleTimelinePlayPause}
          onSeek={isAudioReady ? handleTimelineSeek : undefined}
        />
      )}
      <div className="flex flex-1 overflow-hidden">
        <TranscriptPanel
          transcripts={meetingData.transcripts}
          customPrompt={customPrompt}
          onPromptChange={handlePromptChange}
          onCopyTranscript={copyOperations.handleCopyTranscript}
          onOpenMeetingFolder={meetingOperations.handleOpenMeetingFolder}
          isRecording={isRecording}
          disableAutoScroll={true}
          // Pagination props for efficient loading
          usePagination={true}
          segments={segments}
          hasMore={hasMore}
          isLoadingMore={isLoadingMore}
          totalCount={totalCount}
          loadedCount={loadedCount}
          onLoadMore={onLoadMore}
          // Retranscription props
          meetingId={meeting.id}
          meetingFolderPath={hasSavedAudio ? meeting.folder_path : null}
          showSpeakerAttribution={showSpeakerAttribution}
          activeTime={isAudioReady ? audioPlayer.currentTime : undefined}
          onSeekToTime={isAudioReady ? handleTimelineSeek : undefined}
          onRefetchTranscripts={onRefetchTranscripts}
          onUpdateTranscriptSpeaker={onUpdateTranscriptSpeaker}
          onApplySpeakerToMatching={onApplySpeakerToMatching}
        />
        <SummaryPanel
          meeting={meeting}
          meetingTitle={meetingData.meetingTitle}
          onTitleChange={meetingData.handleTitleChange}
          isEditingTitle={meetingData.isEditingTitle}
          onStartEditTitle={() => meetingData.setIsEditingTitle(true)}
          onFinishEditTitle={() => meetingData.setIsEditingTitle(false)}
          isTitleDirty={meetingData.isTitleDirty}
          summaryRef={meetingData.blockNoteSummaryRef}
          isSaving={meetingData.isSaving}
          onSaveAll={meetingData.saveAllChanges}
          onCopySummary={copyOperations.handleCopySummary}
          onOpenFolder={meetingOperations.handleOpenMeetingFolder}
          aiSummary={meetingData.aiSummary}
          summaryStatus={summaryGeneration.summaryStatus}
          transcripts={meetingData.transcripts}
          modelConfig={modelConfig}
          setModelConfig={setModelConfig}
          onSaveModelConfig={handleSaveModelConfig}
          onGenerateSummary={summaryGeneration.handleGenerateSummary}
          onStopGeneration={summaryGeneration.handleStopGeneration}
          customPrompt={customPrompt}
          summaryResponse={summaryResponse}
          onSaveSummary={meetingData.handleSaveSummary}
          onSummaryChange={meetingData.handleSummaryChange}
          onDirtyChange={meetingData.setIsSummaryDirty}
          summaryError={summaryGeneration.summaryError}
          onRegenerateSummary={summaryGeneration.handleRegenerateSummary}
          getSummaryStatusMessage={summaryGeneration.getSummaryStatusMessage}
          availableTemplates={templates.availableTemplates}
          selectedTemplate={templates.selectedTemplate}
          onTemplateSelect={templates.handleTemplateSelection}
          isModelConfigLoading={false}
          onOpenModelSettings={handleRegisterModalOpen}
        />
      </div>
      <MeetingChat
        meetingId={meeting.id}
        provider={modelConfig?.provider}
        model={modelConfig?.model}
      />
    </motion.div>
  );
}
