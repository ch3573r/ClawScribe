"use client";

import { Transcript, TranscriptSegmentData } from '@/types';
import { VirtualizedTranscriptView } from '@/components/VirtualizedTranscriptView';
import { TranscriptButtonGroup } from './TranscriptButtonGroup';
import { useMemo, useState, useEffect } from 'react';
import { TranscriptCorrections } from './TranscriptCorrections';

interface TranscriptPanelProps {
  focusedSource?: { id: string; request: number };
  transcripts: Transcript[];
  customPrompt: string;
  onPromptChange: (value: string) => void;
  onCopyTranscript: () => void;
  onOpenMeetingFolder: () => Promise<void>;
  isRecording: boolean;
  disableAutoScroll?: boolean;

  // Optional pagination props (when using virtualization)
  usePagination?: boolean;
  segments?: TranscriptSegmentData[];
  hasMore?: boolean;
  isLoadingMore?: boolean;
  totalCount?: number;
  loadedCount?: number;
  onLoadMore?: () => void;

  // Retranscription props
  meetingId?: string;
  meetingFolderPath?: string | null;
  showSpeakerAttribution?: boolean;
  activeTime?: number;
  onSeekToTime?: (seconds: number) => void;
  onRefetchTranscripts?: () => Promise<void>;
  onUpdateTranscriptSpeaker?: (transcriptId: string, speaker: string | null) => Promise<void>;
  onApplySpeakerToMatching?: (fromSpeaker: string | null | undefined, speaker: string | null) => Promise<number>;
}

export function TranscriptPanel({
  focusedSource,
  transcripts,
  customPrompt,
  onPromptChange,
  onCopyTranscript,
  onOpenMeetingFolder,
  isRecording,
  disableAutoScroll = false,
  usePagination = false,
  segments,
  hasMore,
  isLoadingMore,
  totalCount,
  loadedCount,
  onLoadMore,
  meetingId,
  meetingFolderPath,
  showSpeakerAttribution = true,
  activeTime,
  onSeekToTime,
  onRefetchTranscripts,
  onUpdateTranscriptSpeaker,
  onApplySpeakerToMatching,
}: TranscriptPanelProps) {
  const [editing, setEditing] = useState<TranscriptSegmentData | null>(null);
  const [replaceOpen, setReplaceOpen] = useState(false);
  useEffect(() => { setEditing(null); setReplaceOpen(false); }, [meetingId]);
  // Convert transcripts to segments if pagination is not used but we want virtualization
  const convertedSegments = useMemo(() => {
    if (usePagination && segments) {
      return segments;
    }
    // Convert transcripts to segments for virtualization
    return transcripts.map(t => ({
      id: t.id,
      timestamp: t.audio_start_time ?? 0,
      endTime: t.audio_end_time,
      text: t.text,
      original_text: t.original_text,
      confidence: t.confidence,
      speaker: t.speaker,
    }));
  }, [transcripts, usePagination, segments]);

  return (
    <div data-transcript-panel tabIndex={0} onKeyDown={event => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'h') { event.preventDefault(); event.stopPropagation(); setReplaceOpen(true); }
    }} className="flex h-[26rem] min-h-[18rem] min-w-0 md:h-auto shrink-0 flex-col border-r border-border bg-card md:w-[32%] xl:w-[30rem] 2xl:w-[32rem]">
      {/* Title area */}
      <div className="border-b border-border px-3 py-2">
        <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-2">
          <div className="min-w-0 flex items-baseline gap-2">
            <h2 className="text-sm font-semibold text-foreground">Transcript</h2>
            <span className="text-xs text-muted-foreground">
              {usePagination ? (totalCount ?? convertedSegments.length) : (transcripts?.length || 0)} segments
            </span>
          </div>
          <TranscriptButtonGroup
            transcriptCount={usePagination ? (totalCount ?? convertedSegments.length) : (transcripts?.length || 0)}
            onCopyTranscript={onCopyTranscript}
            onOpenMeetingFolder={onOpenMeetingFolder}
            meetingId={meetingId}
            meetingFolderPath={meetingFolderPath}
            showSpeakerAttribution={showSpeakerAttribution}
            onRefetchTranscripts={onRefetchTranscripts}
          />
        </div>
      </div>

      {meetingId && <TranscriptCorrections key={meetingId} meetingId={meetingId} editing={editing} onClose={() => setEditing(null)}
        onChanged={onRefetchTranscripts} replaceOpen={replaceOpen} onReplaceOpenChange={setReplaceOpen} />}
      {/* Transcript content - use virtualized view for better performance */}
      <div className="flex-1 overflow-hidden pb-4">
        <VirtualizedTranscriptView
          focusedSource={focusedSource}
          onEditSegment={meetingId && !isRecording ? setEditing : undefined}
          segments={convertedSegments}
          isRecording={isRecording}
          isPaused={false}
          isProcessing={false}
          isStopping={false}
          enableStreaming={false}
          showConfidence={true}
          disableAutoScroll={disableAutoScroll}
          hasMore={hasMore}
          isLoadingMore={isLoadingMore}
          totalCount={totalCount}
          loadedCount={loadedCount}
          onLoadMore={onLoadMore}
          showSpeakerLabels={showSpeakerAttribution}
          activeTime={activeTime}
          onSeekToTime={onSeekToTime}
          onSpeakerChange={showSpeakerAttribution ? onUpdateTranscriptSpeaker : undefined}
          onApplySpeakerToMatching={showSpeakerAttribution ? onApplySpeakerToMatching : undefined}
        />
      </div>

      {/* Custom prompt input at bottom of transcript section */}
      {!isRecording && convertedSegments.length > 0 && (
        <div className="border-t border-border p-2">
          <textarea
            placeholder="Add context for AI summary. For example people involved, meeting overview, objective etc..."
            className="min-h-[80px] w-full resize-y rounded-md border border-input bg-background px-3 py-2 text-sm shadow-sm focus:border-ring focus:outline-none focus:ring-1 focus:ring-ring"
            value={customPrompt}
            onChange={(e) => onPromptChange(e.target.value)}
          />
        </div>
      )}
    </div>
  );
}
