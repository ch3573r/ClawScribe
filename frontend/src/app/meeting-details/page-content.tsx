"use client";
import { useState, useEffect, useRef, useCallback } from 'react';
import { getMeetingContext, setMeetingContext } from '@/lib/meetingContext';
import { motion } from 'framer-motion';
import type { MeetingDetails, MeetingSummary, TranscriptSegmentData } from '@/types';
import Analytics from '@/lib/analytics';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { TranscriptPanel } from '@/components/MeetingDetails/TranscriptPanel';
import { SummaryPanel } from '@/components/MeetingDetails/SummaryPanel';
import { MeetingHeader } from '@/components/MeetingDetails/MeetingHeader';
import { SavedMeetingAttendance } from '@/components/MeetingAttendance';
import { EvidenceRoute } from '@/components/Knowledge/EvidenceRoute';
import { SavedSearchRoute } from '@/components/MeetingDetails/SavedSearchRoute';
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
import type { TranscriptTextPatch } from '@/lib/meetingTranscripts';
import { usePlaybackShortcuts } from '@/hooks/usePlaybackShortcuts';
import { stepPlaybackRate } from '@/lib/playback-controls';
import type { MeetingBookmark } from '@/lib/library';
import { shouldShowSpeakerAttribution } from '@/lib/speaker-attribution';
import { useSpeakerLabelVisibility } from '@/hooks/useSpeakerLabelVisibility';
import { Button } from '@/components/ui/button';
import Link from 'next/link';
import { ChevronLeft } from 'lucide-react';
import { isActiveSummary, type SummaryJobSnapshot } from '@/lib/summary-job-status';
import { SERIES_REVIEW_CHANGED, type SeriesPreferences } from '@/lib/meeting-follow-through';
import { MEETING_SERIES_TEMPLATE_CHANGED } from '@/lib/meetingCalendarStore';

export default function PageContent({
  meeting,
  evidenceToken,
  savedSourceToken,
  fromArchive,
  archiveAction,
  actionId,
  summaryData,
  summaryJob,
  onMeetingUpdated,
  onRefetchTranscripts,
  onPatchTranscriptText,
  onRevealTranscript,
  onUpdateTranscriptSpeaker,
  onApplySpeakerToMatching,
  // Pagination props for efficient transcript loading
  segments,
  timelineSegments,
  timelineComplete,
  isLoadingTranscript,
  hasMore,
  isLoadingMore,
  totalCount,
  emptyLiveMessage,
  loadedCount,
  onLoadMore,
}: {
  meeting: MeetingDetails;
  evidenceToken?: string | null;
  savedSourceToken?: string | null;
  fromArchive?: boolean;
  archiveAction?: string | null;
  actionId?: string | null;
  summaryData: MeetingSummary | null;
  summaryJob: SummaryJobSnapshot | null;
  onMeetingUpdated?: () => Promise<void>;
  onRevealTranscript?: (id: string, index: number) => Promise<void>;
  onRefetchTranscripts?: () => Promise<void>;
  onPatchTranscriptText?: (patches: TranscriptTextPatch[]) => void;
  onUpdateTranscriptSpeaker?: (transcriptId: string, speaker: string | null) => Promise<void>;
  onApplySpeakerToMatching?: (fromSpeaker: string | null | undefined, speaker: string | null) => Promise<number>;
  // Pagination props
  segments?: TranscriptSegmentData[];
  timelineSegments?: TranscriptSegmentData[];
  timelineComplete?: boolean;
  isLoadingTranscript?: boolean;
  hasMore?: boolean;
  isLoadingMore?: boolean;
  totalCount?: number;
  emptyLiveMessage?: string | null;
  loadedCount?: number;
  onLoadMore?: () => void;
}) {
  const [focusedBookmark, setFocusedBookmark] = useState<string | null>(null);
  const [seriesDefaults, setSeriesDefaults] = useState<{ meetingId: string; value: SeriesPreferences | null } | null>(null);
  const series = seriesDefaults?.meetingId === meeting.id ? seriesDefaults.value : null;
  useEffect(() => {
    let revision = 0;
    const load = () => {
      const request = ++revision;
      setSeriesDefaults(null);
      void invoke<SeriesPreferences | null>('get_meeting_series_preferences', { meetingId: meeting.id }).then(value => {
        if (request === revision) setSeriesDefaults({ meetingId: meeting.id, value });
      }).catch(() => {});
    };
    load();
    window.addEventListener(SERIES_REVIEW_CHANGED, load);
    window.addEventListener(MEETING_SERIES_TEMPLATE_CHANGED, load);
    return () => {
      revision++;
      window.removeEventListener(SERIES_REVIEW_CHANGED, load);
      window.removeEventListener(MEETING_SERIES_TEMPLATE_CHANGED, load);
    };
  }, [meeting.id]);
  console.log('📄 PAGE CONTENT: Initializing with data:', {
    meetingId: meeting.id,
    hasSummary: !!summaryData,
    transcriptsCount: meeting.transcripts?.length
  });

  // State — "Add context" is persisted per meeting so it survives reopening and
  // is applied on every generate/regenerate.
  const [customPrompt, setCustomPrompt] = useState('');
  const [contextLoading, setContextLoading] = useState(true);
  const contextMeeting = useRef(meeting.id);
  contextMeeting.current = meeting.id;
  useEffect(() => {
    let active = true;
    setCustomPrompt('');
    setContextLoading(true);
    void getMeetingContext(meeting.id).then(context => {
      if (active) setCustomPrompt(context);
    }).catch(() => {
      if (active) toast.error('Could not load meeting context. Reopen the meeting to retry.');
    }).finally(() => { if (active) setContextLoading(false); });
    return () => { active = false; };
  }, [meeting.id]);
  const handlePromptChange = useCallback((value: string): void => {
    setCustomPrompt(value);
    void setMeetingContext(meeting.id, value).catch(() => {
      if (contextMeeting.current === meeting.id) toast.error('Could not save meeting context', {
        action: { label: 'Retry', onClick: () => handlePromptChange(value) },
      });
    });
  }, [meeting.id]);
  const [isRecording] = useState(false);
  const [focusedSource, setFocusedSource] = useState<{ id: string; request: number }>();
  const evidenceFocusRequest = useRef(0);
  const reviewRef = useRef<HTMLDivElement>(null);
  const [audioPath, setAudioPath] = useState<string | null>(null);
  const [audioStatus, setAudioStatus] = useState<'loading' | 'found' | 'missing'>('loading');
  const [audioCheck, setAudioCheck] = useState(0);
  const audioPlayer = useAudioPlayer(audioPath);
  const { seek, play, pause, isPlaying } = audioPlayer;
  // Review tools: follow-along, bookmarks on the timeline, and requests raised by keyboard shortcuts.
  const [followPlayback, setFollowPlayback] = useState(true);
  const [bookmarks, setBookmarks] = useState<MeetingBookmark[]>([]);
  const [findOpenRequest, setFindOpenRequest] = useState(0);
  const [bookmarkRequest, setBookmarkRequest] = useState(0);
  const stopFollowing = useCallback(() => setFollowPlayback(false), []);
  const isAudioReady = Boolean(audioStatus === 'found' && audioPath && audioPlayer.duration > 0 && !audioPlayer.error);
  const revealTranscriptPane = useCallback((keepFocus = false) => {
    const review = reviewRef.current;
    const panel = review?.querySelector<HTMLElement>('[data-transcript-panel]');
    if (!review || !panel) return false;
    if (keepFocus && document.activeElement !== panel) return false;
    const scrolls = getComputedStyle(review).overflowY === 'auto';
    if (scrolls) {
      // The virtual list reveals the row inside the pane. Bring the whole pane
      // into the available view first so the row can land anywhere within it.
      review.scrollTop += panel.getBoundingClientRect().top - review.getBoundingClientRect().top;
    }
    if (!keepFocus) panel.focus({ preventScroll: scrolls });
    return true;
  }, []);
  useEffect(() => {
    // The playback strip resizes the virtual list after a cold audio load.
    if (isAudioReady && revealTranscriptPane(true)) {
      const request = ++evidenceFocusRequest.current;
      setFocusedSource(source => source && { id: source.id, request });
    }
  }, [isAudioReady, revealTranscriptPane]);
  const revealEvidence = useCallback(async (id: string, index: number, current: () => boolean) => {
    if (!onRevealTranscript || !current()) throw new Error('The transcript is not ready. Retry the citation.');
    await onRevealTranscript(id, index);
    if (!current()) return;
    setFocusedSource({ id, request: ++evidenceFocusRequest.current });
    revealTranscriptPane();
  }, [onRevealTranscript, revealTranscriptPane]);
  const playEvidence = useCallback(async (seconds: number, current: () => boolean) => {
    if (!current()) return;
    if (!isAudioReady) throw new Error('Saved audio is missing or not ready. The transcript can still be reviewed.');
    await seek(seconds);
    if (!current()) return;
    const played = await play();
    if (!current()) return;
    if (played === false) throw new Error('Saved audio could not play. Retry playback or review the transcript.');
  }, [isAudioReady, seek, play]);

  // Ref to store the modal open function from SummaryGeneratorButtonGroup
  const openModelSettingsRef = useRef<(() => void) | null>(null);

  // Sidebar context

  // Get model config from ConfigContext
  const { modelConfig, setModelConfig } = useConfig();
  const sourceAttributionEnabled = useSourceAttribution();
  // Audio actions operate on the recording folder. The player/timeline waits for
  // a resolved media file, which can lag behind freshly saved recordings.
  const hasRecordingFolder = Boolean(meeting.folder_path);
  useEffect(() => {
    if (archiveAction === 'summary') {
      document.getElementById('meeting-summary-panel')?.focus();
    } else if (archiveAction === 'files') {
      document.querySelector<HTMLElement>('[data-transcript-panel]')?.focus();
    }
  }, [archiveAction]);

  // Custom hooks
  const meetingData = useMeetingData({ meeting, summaryData, onMeetingUpdated });
  const editorRef = meetingData.blockNoteSummaryRef;
  const getSummaryMarkdown = useCallback(async () => (await editorRef.current?.getMarkdown()) ?? '', [editorRef]);
  const hasSpeakerAttribution = shouldShowSpeakerAttribution(
    sourceAttributionEnabled, hasRecordingFolder, timelineSegments, segments, meetingData.transcripts,
  );
  const showSpeakerAttribution = useSpeakerLabelVisibility(meeting.id, hasSpeakerAttribution);
  const templates = useTemplates(meeting.id);

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

      // Emit event so ConfigContext and other listeners stay in sync. A typed key
      // is stored now and is not shared.
      const { emit } = await import('@tauri-apps/api/event');
      await emit('model-config-updated', { ...config, apiKey: null, customOpenAIApiKey: null });

      toast.success('Model settings saved successfully');
    } catch (error) {
      console.error('Failed to save model config:', error);
      toast.error('Failed to save model settings');
    }
  };

  const summaryGeneration = useSummaryGeneration({
    hasExistingSummary: !!meetingData.aiSummary,
    beforeGeneration: meetingData.flushPendingChanges,
    meeting,
    modelConfig: modelConfig,
    isModelConfigLoading: false, // ConfigContext loads on mount
    selectedTemplate: templates.requestedTemplate,
    includeSpeakerLabels: showSpeakerAttribution,
    onMeetingUpdated,
    updateMeetingTitle: meetingData.updateMeetingTitle,
    setAiSummary: meetingData.setAiSummary,
    onOpenModelSettings: handleOpenModelSettings,
  });
  const summaryBusy = ['processing', 'summarizing', 'regenerating'].includes(summaryGeneration.summaryStatus);

  // A summary is already running for this meeting: show it as generating and
  // take its result, instead of offering Generate.
  const summaryRunning = isActiveSummary(summaryJob?.status);
  const attachSummary = summaryGeneration.attachToRunningSummary;
  useEffect(() => {
    if (summaryRunning && !isActiveSummary(summaryGeneration.summaryStatus)) {
      attachSummary(Boolean(summaryData));
    }
    // A listener owns progress through the terminal status.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [summaryRunning, meeting.id]);

  const copyOperations = useCopyOperations({
    meeting,
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
    setAudioStatus('loading');
    setAudioPath(null);

    if (!folderPath) {
      setAudioStatus('missing');
      return;
    }

    invoke<string | null>('resolve_meeting_audio_file', { meetingId: meeting.id })
      .then((path) => {
        if (!cancelled) {
          setAudioPath(path);
          setAudioStatus(path ? 'found' : 'missing');
        }
      })
      .catch((error) => {
        console.warn('Could not resolve meeting audio file:', error);
        if (!cancelled) {
          setAudioPath(null);
          setAudioStatus('missing');
        }
      });

    return () => {
      cancelled = true;
    };
  }, [meeting.id, meeting.folder_path, audioCheck]);

  const handleTimelineSeek = useCallback((seconds: number) => {
    void seek(seconds);
  }, [seek]);

  const playPassage = useCallback((seconds: number) => {
    const reportFailure = () => toast.error('Could not play that part of the recording.');
    void seek(seconds).then(() => play()).then(played => {
      if (played === false) reportFailure();
    }, reportFailure);
  }, [seek, play]);

  const handleTimelinePlayPause = useCallback(() => {
    if (isPlaying) {
      pause();
    } else {
      void play();
    }
  }, [isPlaying, pause, play]);

  // Space, arrows, speed, follow, bookmark and find. Resolved by playback-controls,
  // which stands down while typing, in dialogs and menus, and on controls that own the key.
  const playerBindings = usePlaybackShortcuts({
    toggle: isAudioReady ? handleTimelinePlayPause : undefined,
    seek: isAudioReady ? ({ seconds }) => { void audioPlayer.seekBy(seconds); } : undefined,
    rate: isAudioReady ? ({ direction }) => audioPlayer.setRate(stepPlaybackRate(audioPlayer.rate, direction)) : undefined,
    follow: isAudioReady ? () => setFollowPlayback(value => !value) : undefined,
    bookmark: isAudioReady ? () => setBookmarkRequest(value => value + 1) : undefined,
    find: () => setFindOpenRequest(value => value + 1),
  });

  // Track page view
  useEffect(() => {
    Analytics.trackPageView('meeting_details');
  }, []);

  // Ctrl/⌘+G — generate / regenerate the summary (dispatched by AppShortcuts).
  useEffect(() => {
    const onGenerate = () => {
      if (!summaryBusy && !summaryRunning && !contextLoading && meetingData.transcripts.length > 0) {
        void summaryGeneration.handleGenerateSummary('');
      }
    };
    window.addEventListener('shortcut:generate-summary', onGenerate);
    return () => window.removeEventListener('shortcut:generate-summary', onGenerate);
  }, [meetingData.transcripts.length, summaryGeneration, summaryBusy, summaryRunning, contextLoading]);

  return (
    <motion.div
      ref={reviewRef}
      initial={{ opacity: 0, y: 20 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3, ease: 'easeOut' }}
      className="meeting-details-review custom-scrollbar flex h-full flex-col bg-background"
    >
      {fromArchive && <div className="shrink-0 px-4 pt-2">
        <Button variant="ghost" size="sm" asChild><Link href="/meetings"><ChevronLeft />{archiveAction === 'actions' ? 'Back to open actions' : 'Back to archive'}</Link></Button>
      </div>}
      <EvidenceRoute token={evidenceToken} meetingId={meeting.id} audioReady={isAudioReady} audioStatus={audioStatus} audioError={audioPlayer.error} onReveal={revealEvidence} onPlay={playEvidence} />
      {savedSourceToken && <SavedSearchRoute key={savedSourceToken} token={savedSourceToken} meetingId={meeting.id} onBookmark={setFocusedBookmark} onPlay={isAudioReady ? playPassage : undefined} />}
      <MeetingHeader meeting={meeting} title={meetingData.meetingTitle} isEditing={meetingData.isEditingTitle}
        initialActionId={archiveAction === 'actions' ? actionId : null}
        initialSeriesOpen={archiveAction === 'series'}
        seriesDestination={series?.destination}
        emailDraft={meetingData.details?.email}
        onStartEditing={() => meetingData.setIsEditingTitle(true)} onFinishEditing={() => meetingData.setIsEditingTitle(false)} onChange={meetingData.handleTitleChange}
        duration={audioPlayer.duration || undefined} segmentCount={totalCount ?? meetingData.transcripts.length}
        disabled={summaryBusy}
        getMarkdown={getSummaryMarkdown} onOpenFolder={meetingOperations.handleOpenMeetingFolder}
        attendance={<SavedMeetingAttendance key={meeting.id} meetingId={meeting.id} createdAt={meeting.created_at}
          durationSeconds={audioPlayer.duration || undefined} disabled={summaryBusy}
          templates={templates.availableTemplates} remembered={templates.seriesTemplate} />} />
      {isAudioReady && (
        <SpeakerLaneTimeline
          bindings={playerBindings}
          segments={timelineSegments ?? []}
          showSpeakerAttribution={showSpeakerAttribution}
          totalCount={totalCount}
          onApplySpeakerToMatching={onApplySpeakerToMatching}
          playbackClock={audioPlayer.clock}
          durationSeconds={audioPlayer.duration || undefined}
          isPlaying={isPlaying}
          isAudioReady={isAudioReady}
          onPlayPause={handleTimelinePlayPause}
          onSeek={isAudioReady ? handleTimelineSeek : undefined}
          rate={audioPlayer.rate}
          onRateChange={audioPlayer.setRate}
          followPlayback={followPlayback}
          onFollowChange={setFollowPlayback}
          bookmarks={bookmarks}
        />
      )}
      <div className="meeting-details-panes flex min-h-0 flex-1 flex-col overflow-y-auto md:flex-row md:overflow-hidden">
        <TranscriptPanel
          emptyLiveMessage={emptyLiveMessage}
          bindings={playerBindings}
          focusedBookmark={focusedBookmark}
          openTranscription={archiveAction === 'transcribe'}
          focusedSource={focusedSource}
          speakerSegments={timelineSegments}
          timelineComplete={timelineComplete}
          sourceAttributionEnabled={sourceAttributionEnabled}
          transcripts={meetingData.transcripts}
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
          meetingFolderPath={hasRecordingFolder ? meeting.folder_path : null}
          audioStatus={audioStatus}
          audioError={audioPlayer.error}
          onRetryAudio={hasRecordingFolder ? () => setAudioCheck(check => check + 1) : undefined}
          isLoadingTranscript={isLoadingTranscript}
          showSpeakerAttribution={showSpeakerAttribution}
          playbackClock={isAudioReady ? audioPlayer.clock : undefined}
          onSeekToTime={isAudioReady ? playPassage : undefined}
          onRefetchTranscripts={onRefetchTranscripts}
          onPatchTranscriptText={onPatchTranscriptText}
          onUpdateTranscriptSpeaker={onUpdateTranscriptSpeaker}
          onApplySpeakerToMatching={onApplySpeakerToMatching}
          onRevealSegment={onRevealTranscript}
          followPlayback={isAudioReady && followPlayback && isPlaying}
          onFollowStop={stopFollowing}
          findOpenRequest={findOpenRequest}
          bookmarkRequest={bookmarkRequest}
          onBookmarksChange={setBookmarks}
        />
        <SummaryPanel
          seriesLanguage={series?.language}
          details={meetingData.details}
          onRevealSource={async (source) => {
            await onRevealTranscript?.(source.transcript_id, source.transcript_index);
            setFocusedSource({ id: source.transcript_id, request: Date.now() });
            revealTranscriptPane();
          }}
          onPlaySource={isAudioReady ? async seconds => { await seek(seconds); if (!isPlaying) await play(); } : undefined}
          meeting={meeting}
          meetingTitle={meetingData.meetingTitle}
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
          onRetryTranslation={summaryGeneration.handleRetryTranslation}
          onStopGeneration={summaryGeneration.handleStopGeneration}
          onPromptChange={handlePromptChange}
          contextLoading={contextLoading}
          customPrompt={customPrompt}
          onSaveSummary={meetingData.handleSaveSummary}
          onSummaryChange={meetingData.handleSummaryChange}
          onSummaryDraft={meetingData.handleSummaryDraft}
          saveState={meetingData.saveState}
          hasPreviousSummary={meetingData.hasPreviousSummary}
          summaryRevision={meetingData.summaryRevision}
          onRestorePrevious={meetingData.restorePreviousSummary}
          summaryError={summaryGeneration.summaryError}
          jobSnapshot={summaryGeneration.jobUpdate ?? summaryJob}
          getSummaryStatusMessage={summaryGeneration.getSummaryStatusMessage}
          availableTemplates={templates.availableTemplates}
          selectedTemplate={templates.selectedTemplate}
          onTemplateSelect={templates.handleTemplateSelection}
          isModelConfigLoading={false}
          onOpenModelSettings={handleRegisterModalOpen}
        />
      </div>

    </motion.div>
  );
}
