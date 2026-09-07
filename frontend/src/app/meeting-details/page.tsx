"use client"
import { useSidebar } from "@/components/Sidebar/SidebarProvider";
import { useState, useEffect, useCallback, useRef, Suspense } from "react";
import { Transcript, Summary } from "@/types";
import PageContent from "./page-content";
import { useRouter, useSearchParams } from "next/navigation";
import Analytics from "@/lib/analytics";
import { invoke } from "@tauri-apps/api/core";
import { LoaderIcon } from "lucide-react";
import { useConfig } from "@/contexts/ConfigContext";
import { usePaginatedTranscripts } from "@/hooks/usePaginatedTranscripts";
import { listen } from '@tauri-apps/api/event';
import { RecordingOutcome, recordingRecoveryMessage } from '@/lib/recording-outcome';

interface MeetingDetailsResponse {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  transcripts: Transcript[];
  folder_path?: string;
}

function MeetingDetailsContent() {
  const searchParams = useSearchParams();
  const meetingId = searchParams.get('id');
  const source = searchParams.get('source'); // Check if navigated from recording
  const { setCurrentMeeting, refetchMeetings, stopSummaryPolling } = useSidebar();
  const { isAutoSummary } = useConfig(); // Get auto-summary toggle state
  const router = useRouter();
  const [meetingDetails, setMeetingDetails] = useState<MeetingDetailsResponse | null>(null);
  const [meetingSummary, setMeetingSummary] = useState<Summary | null>(null);
  const [isSummaryLoading, setIsSummaryLoading] = useState(true);
  const autoGenRequest = useRef(0);
  const autoGenPending = useRef(false);
  const [error, setError] = useState<string | null>(null);
  const [shouldAutoGenerate, setShouldAutoGenerate] = useState<boolean>(false);
  const [hasCheckedAutoGen, setHasCheckedAutoGen] = useState<boolean>(false);
  const [recoveryMessage, setRecoveryMessage] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    const refresh = async () => {
      if (!meetingId) return;
      try {
        const outcome = await invoke<RecordingOutcome | null>('get_recording_outcome', { meetingId });
        if (active) setRecoveryMessage(outcome ? recordingRecoveryMessage(outcome) : null);
      } catch {
        if (active) setRecoveryMessage('Recording status could not be checked. Check the transcript before generating notes.');
      }
    };
    void refresh();
    const subscription = listen('retranscription-complete', () => { void refresh(); });
    return () => { active = false; void subscription.then(unlisten => unlisten()).catch(() => {}); };
  }, [meetingId]);

  // Use pagination hook for efficient transcript loading
  const {
    metadata,
    segments,
    transcripts,
    isLoadingMore,
    hasMore,
    totalCount,
    loadedCount,
    loadMore,
    refetch,
    revealSource,
    updateSpeaker,
    applySpeakerToMatching,
    error: transcriptError,
  } = usePaginatedTranscripts({ meetingId: meetingId || '' });

  // Automatic generation uses only the provider the user has already configured.
  const setupAutoGeneration = useCallback(async () => {
    if (hasCheckedAutoGen || autoGenPending.current) return;
    const request = autoGenRequest.current;
    autoGenPending.current = true;
    try {
      if (source !== 'recording' || !isAutoSummary) return;
      const outcome = await invoke<RecordingOutcome | null>('get_recording_outcome', { meetingId });
      if (request !== autoGenRequest.current || (outcome && recordingRecoveryMessage(outcome))) return;
      const config = await invoke<{ model?: string }>('api_get_model_config');
      if (request === autoGenRequest.current && config?.model) setShouldAutoGenerate(true);
    } catch {
      // A failed readiness check must never start generation or change providers.
      console.error('Could not check automatic summary readiness.');
    } finally {
      if (request === autoGenRequest.current) {
        autoGenPending.current = false;
        setHasCheckedAutoGen(true);
      }
    }
  }, [hasCheckedAutoGen, source, isAutoSummary, meetingId]);

  // Sync meeting metadata from pagination hook to meeting details state
  useEffect(() => {
    if (metadata && (metadata.id !== meetingId || meetingId === 'intro-call')) {
      // If invalid meeting ID, don't sync
      return;
    }

    if (metadata) {

      // Build meeting details from metadata and paginated transcripts
      setMeetingDetails({
        id: metadata.id,
        title: metadata.title,
        created_at: metadata.created_at,
        updated_at: metadata.updated_at,
        transcripts: transcripts, // Paginated transcripts from hook
        folder_path: metadata.folder_path, // For retranscription feature
      });

      // Sync with sidebar context
      setCurrentMeeting({ id: metadata.id, title: metadata.title });
    }
  }, [metadata, transcripts, meetingId, setCurrentMeeting]);

  // Handle transcript loading errors
  useEffect(() => {
    if (transcriptError) {
      console.error("Error loading transcripts:");
      setError(transcriptError);
    }
  }, [transcriptError]);

  // Extract fetchMeetingDetails for use in child components (now refetches via hook)
  const fetchMeetingDetails = useCallback(async () => {
    if (!meetingId || meetingId === 'intro-call') {
      return;
    }

    // The usePaginatedTranscripts hook automatically refetches when meetingId changes
    // This function is kept for compatibility with onMeetingUpdated callback
  }, [meetingId]);

  // Reset states when meetingId changes (prevent race conditions)
  useEffect(() => {
    autoGenRequest.current++;
    autoGenPending.current = false;
    setMeetingDetails(null);
    setMeetingSummary(null);
    setError(null);
    // Reset auto-generation state to allow new meeting to be checked
    setHasCheckedAutoGen(false);
    setShouldAutoGenerate(false);
    return () => { autoGenRequest.current++; };
  }, [meetingId]);

  // Cleanup: Stop polling when navigating away from a meeting
  useEffect(() => {
    return () => {
      if (meetingId) {
        stopSummaryPolling(meetingId);
      }
    };
  }, [meetingId, stopSummaryPolling]);

  useEffect(() => {

    if (!meetingId || meetingId === 'intro-call') {
      console.warn("No valid meeting ID in URL - meetingId:");
      setError("No meeting selected");
      Analytics.trackPageView('meeting_details');
      return;
    }


    setMeetingDetails(null);
    setMeetingSummary(null);
    setIsSummaryLoading(true);
    setError(null);
    let active = true;

    const fetchMeetingSummary = async () => {
      try {
        const summary = await invoke('api_get_summary', {
          meetingId: meetingId,
        }) as any;
        if (!active) return;

        // Check if the summary request failed with 404 or error status, or if no summary exists yet (idle)
        // Note: 'cancelled' and 'failed' statuses can still have data if backup was restored
        if (summary.status === 'idle' || (!summary.data && summary.status === 'error')) {
          console.warn("Meeting summary not found or no summary generated yet:");
          setMeetingSummary(null);
          return;
        }

        const summaryData = summary.data || {};

        // Parse if it's a JSON string (backend may return double-encoded JSON)
        let parsedData = summaryData;
        if (typeof summaryData === 'string') {
          try {
            parsedData = JSON.parse(summaryData);
          } catch (e) {
            parsedData = {};
          }
        }


        // Priority 1: BlockNote JSON format
        if (parsedData.summary_json) {
          setMeetingSummary(parsedData as any);
          return;
        }

        // Priority 2: Markdown format
        if (parsedData.markdown) {
          setMeetingSummary(parsedData as any);
          return;
        }

        // Legacy format - apply formatting

        const { MeetingName, _section_order, ...restSummaryData } = parsedData;

        // Format the summary data with consistent styling - PRESERVE ORDER
        const formattedSummary: Summary = {};

        // Use section order if available to maintain exact order and handle duplicates
        const sectionKeys = _section_order || Object.keys(restSummaryData);


        for (const key of sectionKeys) {
          try {
            const section = restSummaryData[key];
            // Comprehensive null checks to prevent the error
            if (section &&
              typeof section === 'object' &&
              'title' in section &&
              'blocks' in section) {
              const typedSection = section as { title?: string; blocks?: any[] };

              // Ensure blocks is an array before mapping
              if (Array.isArray(typedSection.blocks)) {
                formattedSummary[key] = {
                  title: typedSection.title || key,
                  blocks: typedSection.blocks.map((block: any) => ({
                    ...block,
                    // type: 'bullet',
                    color: 'default',
                    content: block?.content?.trim() || ''
                  }))
                };
              } else {
                // Handle case where blocks is not an array
                console.warn("Operation failed; see the application error message.");
                formattedSummary[key] = {
                  title: typedSection.title || key,
                  blocks: []
                };
              }
            } else {
              console.warn("Operation failed; see the application error message.");
            }
          } catch (error) {
            console.warn("Operation failed; see the application error message.");
            // Continue processing other sections
          }
        }

        setMeetingSummary(formattedSummary);
      } catch (error) {
        if (!active) return;
        setHasCheckedAutoGen(true);
        console.error("FETCH SUMMARY: Error fetching meeting summary:");
        // Don't set error state for summary fetch failure, set to null to show generate button
        setMeetingSummary(null);
      } finally {
        if (active) setIsSummaryLoading(false);
      }
    };

    void fetchMeetingSummary();
    return () => { active = false; };
  }, [meetingId]);

  // Auto-generation check: runs when meeting is loaded with no summary
  useEffect(() => {
    const checkAutoGen = async () => {
      // Only auto-generate if:
      // 1. We have meeting details
      // 2. No summary exists
      // 3. Meeting has transcripts
      // 4. Haven't checked yet
      if (
        meetingDetails?.id === meetingId &&
        !isSummaryLoading &&
        meetingSummary === null &&
        meetingDetails.transcripts &&
        meetingDetails.transcripts.length > 0 &&
        !hasCheckedAutoGen
      ) {
        await setupAutoGeneration();
      }
    };

    checkAutoGen();
  }, [meetingDetails, meetingId, meetingSummary, isSummaryLoading, hasCheckedAutoGen, setupAutoGeneration]);

  if (error) {
    return (
      <div className="flex h-full items-center justify-center">
        <div className="text-center">
          <p className="text-red-500 mb-4">{error}</p>
          <button
            onClick={() => router.push('/')}
            className="px-4 py-2 bg-primary text-white rounded hover:bg-primary"
          >
            Go Back
          </button>
        </div>
      </div>
    );
  }

  // Show the full-page spinner only for the initial load. Post-processing
  // refetches must keep PageContent mounted so completion benchmark dialogs
  // are not destroyed as soon as retranscription/diarization finishes.
  if (!meetingDetails || meetingDetails.id !== meetingId) {
    return <div className="flex h-full items-center justify-center">
      <LoaderIcon className="animate-spin size-6 " />
    </div>;
  }

  return <div className="flex h-full min-h-0 flex-col">
    {recoveryMessage && <div role="alert" className="shrink-0 border-b border-amber-500/40 bg-amber-500/10 px-4 py-3 text-sm text-foreground">
      <strong>Meeting needs review. </strong>{recoveryMessage} Automatic notes are paused.
    </div>}
    <div className="min-h-0 flex-1"><PageContent
    key={meetingDetails.id}
    meeting={meetingDetails}
    summaryData={meetingSummary}
    shouldAutoGenerate={shouldAutoGenerate}
    onAutoGenerateComplete={() => setShouldAutoGenerate(false)}
    onMeetingUpdated={async () => {
      // Refetch meeting details to get updated title from backend
      await fetchMeetingDetails();
      // Refetch meetings list to update sidebar
      await refetchMeetings();
    }}
    onRefetchTranscripts={refetch}
    onRevealTranscript={revealSource}
    onUpdateTranscriptSpeaker={updateSpeaker}
    onApplySpeakerToMatching={applySpeakerToMatching}
    // Pagination props for efficient transcript loading
    segments={segments}
    hasMore={hasMore}
    isLoadingMore={isLoadingMore}
    totalCount={totalCount}
    loadedCount={loadedCount}
    onLoadMore={loadMore}
  /></div></div>;
}

export default function MeetingDetails() {
  return (
    <Suspense fallback={
      <div className="flex h-full items-center justify-center">
        <LoaderIcon className="animate-spin size-6" />
      </div>
    }>
      <MeetingDetailsContent />
    </Suspense>
  );
}
