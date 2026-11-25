"use client";

import { Transcript, TranscriptSegmentData } from '@/types';
import { VirtualizedTranscriptView } from '@/components/VirtualizedTranscriptView';
import { TranscriptButtonGroup } from './TranscriptButtonGroup';
import { useMemo, useState, useEffect, useRef, useCallback, type ComponentProps } from 'react';
import { toast } from 'sonner';
import type { MeetingBookmark } from '@/lib/library';
import { findTranscriptMatches, normalizeFindQuery, stepMatch } from '@/lib/transcript-find';
import { TranscriptFind } from './TranscriptFind';
import { TranscriptCorrections } from './TranscriptCorrections';
import { MeetingBookmarks } from './MeetingBookmarks';
import { Button } from '@/components/ui/button';
import type { PlayerBindings } from '@/lib/player-bindings';
import type { TranscriptTextPatch } from '@/lib/meetingTranscripts';
import { usePlaybackTime, type PlaybackClock } from '@/hooks/useAudioPlayer';
import { hasSavedSpeakerLabels } from '@/lib/speaker-attribution';

interface TranscriptPanelProps {
  bindings?: PlayerBindings;
  focusedBookmark?: string | null;
  openTranscription?: boolean;
  speakerSegments?: TranscriptSegmentData[];
  timelineComplete?: boolean;
  sourceAttributionEnabled?: boolean;
  attendeeNames?: readonly string[];
  highlightedSpeaker?: string | null;
  onClearSpeakerHighlight?: () => void;
  focusedSource?: { id: string; request: number };
  transcripts: Transcript[];
  onCopyTranscript: () => void;
  onOpenMeetingFolder: () => Promise<void>;
  isRecording: boolean;
  disableAutoScroll?: boolean;

  // Optional pagination props (when using virtualization)
  usePagination?: boolean;
  segments?: TranscriptSegmentData[];
  hasMore?: boolean;
  isLoadingMore?: boolean;
  isLoadingTranscript?: boolean;
  totalCount?: number;
  emptyLiveMessage?: string | null;
  loadedCount?: number;
  onLoadMore?: () => void;

  // Retranscription props
  meetingId?: string;
  meetingFolderPath?: string | null;
  audioStatus: 'loading' | 'found' | 'missing';
  audioUnavailableReason?: string | null;
  audioError?: string | null;
  onRetryAudio?: () => void;
  showSpeakerAttribution?: boolean;
  activeTime?: number;
  playbackClock?: PlaybackClock;
  onSeekToTime?: (seconds: number) => void;
  onRefetchTranscripts?: () => Promise<void>;
  onPatchTranscriptText?: (patches: TranscriptTextPatch[]) => void;
  onUpdateTranscriptSpeaker?: (transcriptId: string, speaker: string | null) => Promise<void>;
  onApplySpeakerToMatching?: (fromSpeaker: string | null | undefined, speaker: string | null) => Promise<number>;

  // Review tools
  onRevealSegment?: (id: string, index: number) => Promise<void>;
  followPlayback?: boolean;
  onFollowStop?: () => void;
  /** Counters raised by page shortcuts: open the find bar or the new-bookmark dialog. */
  findOpenRequest?: number;
  bookmarkRequest?: number;
  onBookmarksChange?: (bookmarks: MeetingBookmark[]) => void;
}

export function TranscriptPanel({
  bindings,
  focusedBookmark,
  openTranscription = false,
  focusedSource,
  speakerSegments,
  timelineComplete,
  sourceAttributionEnabled,
  attendeeNames,
  highlightedSpeaker,
  onClearSpeakerHighlight,
  transcripts,
  onCopyTranscript,
  onOpenMeetingFolder,
  isRecording,
  disableAutoScroll = false,
  usePagination = false,
  segments,
  hasMore,
  isLoadingMore,
  isLoadingTranscript,
  totalCount,
  emptyLiveMessage,
  loadedCount,
  onLoadMore,
  meetingId,
  meetingFolderPath,
  audioStatus,
  audioUnavailableReason = null,
  audioError,
  onRetryAudio,
  showSpeakerAttribution = true,
  activeTime: initialTime,
  playbackClock,
  onSeekToTime,
  onRefetchTranscripts,
  onPatchTranscriptText,
  onUpdateTranscriptSpeaker,
  onApplySpeakerToMatching,
  onRevealSegment,
  followPlayback = false,
  onFollowStop,
  findOpenRequest = 0,
  bookmarkRequest = 0,
  onBookmarksChange,
}: TranscriptPanelProps) {
  const activeTime = usePlaybackTime(playbackClock) ?? initialTime;
  const panelRef = useRef<HTMLDivElement>(null);
  const [bookmarkOpen, setBookmarkOpen] = useState(false);
  const [findOpen, setFindOpen] = useState(false);
  const [findQuery, setFindQuery] = useState('');
  const [findIndex, setFindIndex] = useState(-1);
  const [findFocus, setFindFocus] = useState(0);
  const [findScroll, setFindScroll] = useState(0);
  const findStep = useRef(0);
  const openFind = useCallback(() => { setFindOpen(true); setFindFocus(value => value + 1); }, []);
  useEffect(() => { if (findOpenRequest > 0 && meetingId) openFind(); }, [findOpenRequest]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => { if (bookmarkRequest > 0 && meetingId) setBookmarkOpen(true); }, [bookmarkRequest]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => { setFindOpen(false); setFindQuery(''); setFindIndex(-1); }, [meetingId]);
  const [editing, setEditing] = useState<TranscriptSegmentData | null>(null);
  const [replaceOpen, setReplaceOpen] = useState(false);
  const openReplace = () => setReplaceOpen(true);
  const [transcribeOpen, setTranscribeOpen] = useState(false);
  useEffect(() => { setEditing(null); setReplaceOpen(false); setTranscribeOpen(false); }, [meetingId]);
  useEffect(() => {
    if (openTranscription && meetingFolderPath && audioStatus === 'found') setTranscribeOpen(true);
  }, [meetingId, openTranscription, meetingFolderPath, audioStatus]);
  // Convert transcripts to segments if pagination is not used but we want virtualization
  const convertedSegments = useMemo(() => {
    if (usePagination && segments) {
      return segments;
    }
    // Convert transcripts to segments for virtualization
    return transcripts.map(t => ({
      id: t.id,
      timestamp: t.audio_start_time ?? Number.NaN,
      endTime: t.audio_end_time,
      text: t.text,
      original_text: t.original_text,
      confidence: t.confidence,
      speaker: t.speaker,
    }));
  }, [transcripts, usePagination, segments]);

  // The speaker timeline contains the same saved rows as the paged transcript.
  // Once it is complete, playhead seeks can reach every row in the virtual list.
  const completeTimeline = timelineComplete ? speakerSegments : undefined;
  const displayedSegments = useMemo(() => {
    if (!completeTimeline) return convertedSegments;
    const hydrated = new Map(convertedSegments.map(row => [row.id, row]));
    return completeTimeline.map(row => hydrated.get(row.id) ?? row);
  }, [completeTimeline, convertedSegments]);
  const editSegment = useCallback((id: string) => {
    const segment = displayedSegments.find(row => row.id === id);
    if (segment) setEditing(segment);
  }, [displayedSegments]);
  const searchable = completeTimeline ?? (speakerSegments?.length ? speakerSegments : convertedSegments);
  const searchComplete = !usePagination || timelineComplete === true ||
    (totalCount !== undefined && searchable.length === totalCount);
  const hasSpeakerLabels = useMemo(() => hasSavedSpeakerLabels(speakerSegments, displayedSegments), [speakerSegments, displayedSegments]);
  const canEditSpeakers = Boolean(meetingId && !isRecording && onUpdateTranscriptSpeaker);
  const findMatches = useMemo(() => findOpen ? findTranscriptMatches(searchable, findQuery) : [], [findOpen, searchable, findQuery]);
  const matchesRef = useRef(findMatches);
  matchesRef.current = findMatches;
  // The page's reveal function changes whenever pages load; searching must not restart because of that.
  const revealRef = useRef(onRevealSegment);
  revealRef.current = completeTimeline ? undefined : onRevealSegment;
  const currentMatchId = useRef<string | null>(null);
  const goToMatch = useCallback(async (index: number) => {
    const step = ++findStep.current;
    const match = matchesRef.current[index];
    if (!match) { currentMatchId.current = null; setFindIndex(-1); return; }
    try { await revealRef.current?.(match.id, match.index); }
    catch { if (step === findStep.current) toast.error('Could not load that part of the transcript. Try again.'); return; }
    if (step !== findStep.current) return;
    currentMatchId.current = match.id;
    setFindIndex(index);
    setFindScroll(value => value + 1);
  }, []);
  // A new search starts at its first match after a short pause in typing.
  useEffect(() => {
    if (!findOpen) return;
    const timer = setTimeout(() => { void goToMatch(0); }, 150);
    return () => clearTimeout(timer);
  }, [findOpen, findQuery, goToMatch]);
  // When the transcript changes under an open search (a correction, a speaker rename), stay on the same passage.
  useEffect(() => {
    if (!findOpen) return;
    setFindIndex(findMatches.findIndex(match => match.id === currentMatchId.current));
  }, [findOpen, findMatches]);
  const closeFind = useCallback(() => {
    findStep.current++;
    currentMatchId.current = null;
    setFindOpen(false); setFindIndex(-1);
    document.querySelector<HTMLElement>('[data-transcript-panel]')?.focus();
  }, []);
  const activeFindId = findOpen && findIndex >= 0 ? findMatches[findIndex]?.id ?? null : null;

  const transcriptCount = usePagination ? (totalCount ?? convertedSegments.length) : (transcripts?.length || 0);
  const emptyState = transcriptCount === 0 ? (
    <div className="space-y-3 text-sm">
      {isLoadingTranscript || audioStatus === 'loading' ? (
        <p role="status">Loading meeting…</p>
      ) : !meetingFolderPath ? (
        <p>No transcript or recording is attached to this meeting.</p>
      ) : audioStatus === 'found' ? (
        <>
          <h3 className="text-lg font-semibold text-foreground">No transcript yet</h3>
          <p>{emptyLiveMessage ?? "This meeting's audio is saved but hasn't been transcribed."}</p>
          <Button onClick={() => setTranscribeOpen(true)}>{emptyLiveMessage ? 'Retranscribe' : 'Transcribe now'}</Button>
        </>
      ) : (
        <>
          <p>{emptyLiveMessage ? `${emptyLiveMessage} No audio file was found in this meeting's folder.`
            : "No transcript yet, and no audio file was found in this meeting's folder."}</p>
          <Button variant="outline" onClick={onOpenMeetingFolder}>Open recording folder</Button>
        </>
      )}
    </div>
  ) : undefined;

  const renderToolbar = (actions?: ComponentProps<typeof TranscriptButtonGroup>['correctionActions']) => (
    <div className="flex shrink-0 items-center justify-between gap-1 border-b border-border px-3 py-2">
        <h2 className="text-sm font-semibold text-foreground">Transcript</h2>
        <TranscriptButtonGroup bindings={bindings} transcriptCount={transcriptCount} transcribeOpen={transcribeOpen} onTranscribeOpenChange={setTranscribeOpen}
          onCopyTranscript={onCopyTranscript} onOpenMeetingFolder={onOpenMeetingFolder} meetingId={meetingId} meetingFolderPath={meetingFolderPath}
          audioAvailable={audioStatus === 'found'} hasSpeakerLabels={hasSpeakerLabels} speakerLabelsComplete={searchComplete}
          onRefetchTranscripts={onRefetchTranscripts} onFindReplace={meetingId ? openReplace : undefined}
          onFind={meetingId ? openFind : undefined}
          onAddBookmark={meetingId ? () => setBookmarkOpen(true) : undefined} correctionActions={actions} />
      </div>
  );

  return (
    <div ref={panelRef} data-transcript-panel tabIndex={0} onKeyDown={event => {
      if (meetingId && transcriptCount > 0 && (event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'h') { event.preventDefault(); event.stopPropagation(); openReplace(); }
    }} className="flex h-[26rem] min-h-[18rem] min-w-0 md:h-auto shrink-0 flex-col border-r border-border bg-card md:w-[38%] md:min-w-[18rem] md:max-w-[30rem] xl:w-[clamp(420px,36%,560px)] xl:min-w-[420px] xl:max-w-[560px] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring">
      {meetingId ? <TranscriptCorrections key={meetingId} meetingId={meetingId} editing={editing} onClose={() => setEditing(null)}
        onChanged={patches => patches && onPatchTranscriptText ? onPatchTranscriptText(patches) : onRefetchTranscripts?.()}
        replaceOpen={replaceOpen} replaceQuery={findQuery} onReplaceOpenChange={setReplaceOpen}
        renderActions={renderToolbar} /> : renderToolbar()}
      {findOpen && <TranscriptFind bindings={bindings} query={findQuery} onQueryChange={setFindQuery} current={findIndex} total={findMatches.length} complete={searchComplete} focusRequest={findFocus}
        onStep={direction => void goToMatch(stepMatch(findIndex, findMatches.length, direction))} onClose={closeFind}
        onReplace={meetingId && transcriptCount > 0 ? openReplace : undefined} />}
      {highlightedSpeaker !== undefined && onClearSpeakerHighlight && (
        <div className="flex shrink-0 items-center justify-between gap-2 border-b border-border px-3 py-1 text-xs">
          <p role="status" className="min-w-0 truncate" title={highlightedSpeaker ?? undefined}>
            {highlightedSpeaker === null ? 'Highlighting unlabeled segments' : `Highlighting ${highlightedSpeaker}`}
          </p>
          <Button type="button" variant="ghost" size="sm" className="shrink-0" onClick={() => {
            onClearSpeakerHighlight();
            panelRef.current?.focus();
          }}>Clear highlight</Button>
        </div>
      )}
      {transcriptCount > 0 && (audioStatus === 'missing' || audioError) && <div className="flex shrink-0 items-center gap-2 border-b border-border px-3 py-2 text-xs text-muted-foreground">
        <p role="status">{audioError ? 'Saved audio could not be played.' : audioUnavailableReason ?? 'Saved audio is unavailable.'} You can still read and correct the transcript.</p>
        {onRetryAudio && <Button variant="outline" size="sm" onClick={onRetryAudio}>Check again</Button>}
      </div>}
      {meetingId && <MeetingBookmarks key={meetingId} meetingId={meetingId} focusedBookmark={focusedBookmark} currentTime={activeTime} onSeek={onSeekToTime} addOpen={bookmarkOpen} onAddOpenChange={setBookmarkOpen} onBookmarksChange={onBookmarksChange} />}
      {/* Transcript content - use virtualized view for better performance */}
      <div className="flex-1 overflow-hidden pb-4">
        <VirtualizedTranscriptView
          focusedSource={focusedSource}
          speakerSegments={speakerSegments}
          sourceAttributionEnabled={sourceAttributionEnabled}
          attendeeNames={attendeeNames}
          highlightedSpeaker={highlightedSpeaker}
          onEditSegment={meetingId && !isRecording ? editSegment : undefined}
          segments={displayedSegments}
          emptyState={emptyState}
          isRecording={isRecording}
          isPaused={false}
          isProcessing={false}
          isStopping={false}
          enableStreaming={false}
          showConfidence={true}
          disableAutoScroll={disableAutoScroll}
          hasMore={!completeTimeline && hasMore}
          isLoadingMore={!completeTimeline && isLoadingMore}
          totalCount={totalCount}
          loadedCount={completeTimeline?.length ?? loadedCount}
          onLoadMore={onLoadMore}
          showSpeakerLabels={showSpeakerAttribution || canEditSpeakers}
          activeTime={activeTime}
          onSeekToTime={onSeekToTime}
          followPlayback={followPlayback}
          onUserScroll={onFollowStop}
          findQuery={findOpen ? normalizeFindQuery(findQuery) : undefined}
          activeFindId={activeFindId}
          findRequest={findScroll}
          onSpeakerChange={canEditSpeakers ? onUpdateTranscriptSpeaker : undefined}
          onApplySpeakerToMatching={canEditSpeakers ? onApplySpeakerToMatching : undefined}
        />
      </div>
    </div>
  );
}
