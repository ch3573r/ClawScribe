import { useCallback, RefObject } from 'react';
import type { Transcript, MeetingMetadata, MeetingSummary } from '@/types';
import { BlockNoteSummaryViewRef } from '@/components/AISummary/BlockNoteSummaryView';
import { isEditorSummary } from '@/lib/meeting-summary';
import { toast } from 'sonner';
import { invoke as invokeTauri } from '@tauri-apps/api/core';
import { summaryClipboard, transcriptClipboard, writeClipboard } from '@/lib/clipboard-format';
import { fetchAllMeetingTranscripts } from '@/lib/meetingTranscripts';

interface UseCopyOperationsProps {
  meeting: MeetingMetadata;
  meetingTitle: string;
  aiSummary: MeetingSummary | null;
  blockNoteSummaryRef: RefObject<BlockNoteSummaryViewRef | null>;
  includeSpeakerLabels?: boolean;
}

export function useCopyOperations({
  meeting,
  meetingTitle,
  aiSummary,
  blockNoteSummaryRef,
  includeSpeakerLabels = true,
}: UseCopyOperationsProps) {

  // Helper function to fetch ALL transcripts for copying (not just paginated data)
  const fetchAllTranscripts = useCallback(async (meetingId: string): Promise<Transcript[] | null> => {
    try {
      return await fetchAllMeetingTranscripts(invokeTauri, meetingId);
    } catch {
      toast.error('Failed to fetch transcripts for copying');
      return null;
    }
  }, []);

  // Copy transcript to clipboard
  const handleCopyTranscript = useCallback(async () => {
    // CHANGE: Fetch ALL transcripts from database, not from pagination state
    console.log('📊 Fetching all transcripts for copying...');
    const allTranscripts = await fetchAllTranscripts(meeting.id);
    if (allTranscripts === null) return;

    if (!allTranscripts.length) {
      const error_msg = 'No transcripts available to copy';
      console.log(error_msg);
      toast.error(error_msg);
      return;
    }

    console.log(`✅ Copying ${allTranscripts.length} transcripts to clipboard`);

    // Readable text plus escaped HTML; no Markdown markup and no internal meeting id.
    try {
      await writeClipboard(transcriptClipboard({
        title: meetingTitle ?? meeting.title ?? '',
        createdAt: meeting.created_at,
        transcripts: allTranscripts,
        includeSpeaker: includeSpeakerLabels,
      }));
    } catch {
      toast.error('Could not copy the transcript');
      return;
    }
    toast.success("Transcript copied to clipboard");
  }, [meeting, meetingTitle, fetchAllTranscripts, includeSpeakerLabels]);

  // Copy summary to clipboard
  const handleCopySummary = useCallback(async () => {
    try {
      let summaryMarkdown = '';

      console.log('🔍 Copy Summary - Starting...');

      // Try to get markdown from BlockNote editor first
      if (blockNoteSummaryRef.current?.getMarkdown) {
        console.log('📝 Trying to get markdown from ref...');
        summaryMarkdown = await blockNoteSummaryRef.current.getMarkdown();
        console.log('📝 Got markdown from ref, length:', summaryMarkdown.length);
      }

      // Fallback: Check if aiSummary has markdown property
      if (!summaryMarkdown && aiSummary && isEditorSummary(aiSummary)) {
        console.log('📝 Using markdown from aiSummary');
        summaryMarkdown = aiSummary.markdown || '';
        console.log('📝 Markdown from aiSummary, length:', summaryMarkdown.length);
      }

      // Fallback: Check for legacy format
      if (!summaryMarkdown && aiSummary && !isEditorSummary(aiSummary)) {
        summaryMarkdown = Object.values(aiSummary).map(section => {
          const content = section.blocks.map(block => `- ${block.content}`).join('\n');
          return `## ${section.title}\n\n${content}`;
        }).join('\n\n');
      }

      // If still no summary content, show message
      if (!summaryMarkdown.trim()) {
        console.error('❌ No summary content available to copy');
        toast.error('No summary content available to copy');
        return;
      }

      // Readable text plus escaped HTML; no Markdown markup, source anchors or internal meeting id.
      await writeClipboard(summaryClipboard({ title: meetingTitle, createdAt: meeting.created_at, markdown: summaryMarkdown }));

      console.log('✅ Successfully copied to clipboard!');
      toast.success("Summary copied to clipboard");
    } catch (error) {
      console.error('❌ Failed to copy summary:', error);
      toast.error("Failed to copy summary");
    }
  }, [aiSummary, meetingTitle, meeting, blockNoteSummaryRef]);

  return {
    handleCopyTranscript,
    handleCopySummary,
  };
}
