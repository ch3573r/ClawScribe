'use client';

import { useRef, useReducer, useState, startTransition, useEffect, memo, useMemo, useCallback, type ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useAutoScroll } from "@/hooks/useAutoScroll";
import { useTranscriptStreaming } from "@/hooks/useTranscriptStreaming";
import { formatTranscriptDisplayText } from "@/lib/transcriptDisplay";
import { ConfidenceIndicator } from "./ConfidenceIndicator";
import { Tooltip, TooltipContent, TooltipTrigger } from "./ui/tooltip";
import { Button } from "./ui/button";
import { SpeakerLabelPopover, SpeakerDot } from "./MeetingDetails/SpeakerLabelPopover";
import { buildSpeakerCatalog, getSpeakerColor, speakerLabel, type SpeakerDetails } from "@/lib/speaker-labels";
import { BookmarkPlus, Pencil } from "lucide-react";
import { RecordingStatusBar } from "./RecordingStatusBar";
import { motion, AnimatePresence } from "framer-motion";
import { TranscriptSegmentData } from "@/types";
import { splitForHighlight } from "@/lib/transcript-find";
import { activePlaybackSegment, indexPlaybackRanges } from "@/hooks/transcript-playback";
import { usePlaybackTime, type PlaybackClock } from "@/hooks/useAudioPlayer";

export interface VirtualizedTranscriptViewProps {
    speakerSegments?: TranscriptSegmentData[];
    sourceAttributionEnabled?: boolean;
    attendeeNames?: readonly string[];
    highlightedSpeaker?: string | null;
    focusedSource?: { id: string; request: number };
    onEditSegment?: (id: string) => void;
    onBookmarkSegment?: (seconds: number, anchor: HTMLButtonElement) => void;
    /** Transcript segments to display */
    segments: TranscriptSegmentData[];
    /** Saved-meeting empty state; recording keeps its live status. */
    emptyState?: React.ReactNode;
    /** Optional controls and content above the transcript in its scroll area. */
    header?: ReactNode;
    beforeTranscript?: ReactNode;
    contentClassName?: string;
    /** Whether recording is in progress */
    isRecording?: boolean;
    /** Whether recording is paused */
    isPaused?: boolean;
    /** Whether processing/finalizing transcription */
    isProcessing?: boolean;
    /** Whether stopping */
    isStopping?: boolean;
    /** Enable streaming effect for latest segment */
    enableStreaming?: boolean;
    /** Show confidence indicators */
    showConfidence?: boolean;
    /** Completely disable auto-scroll behavior (for meeting details page) */
    disableAutoScroll?: boolean;
    /** Show saved speaker/source labels and editing controls. */
    showSpeakerLabels?: boolean;
    /** Current playback position, in recording-relative seconds. */
    activeTime?: number;
    playbackClock?: PlaybackClock;
    /** Seek recording playback to a transcript timestamp. */
    onSeekToTime?: (seconds: number) => void;
    /** Keep the segment being played in view. */
    followPlayback?: boolean;
    /** The reader scrolled by hand, so following should stop. */
    onUserScroll?: () => void;
    /** Find in transcript: the text to mark, the current match and a request counter that scrolls to it. */
    findQuery?: string;
    activeFindId?: string | null;
    findRequest?: number;

    // Pagination props (infinite scroll)
    hasMore?: boolean;
    isLoadingMore?: boolean;
    totalCount?: number;
    loadedCount?: number;
    onLoadMore?: () => void;

    // Saved meeting speaker review
    onSpeakerChange?: (segmentId: string, speaker: string | null) => Promise<void> | void;
    onApplySpeakerToMatching?: (fromSpeaker: string | null | undefined, speaker: string | null) => Promise<number> | number | void;
}

// Threshold for enabling virtualization (below this, use simple rendering)
const VIRTUALIZATION_THRESHOLD = 10;

// Helper function to format seconds as recording-relative time [MM:SS]
function formatRecordingTime(seconds: number | undefined): string {
    if (seconds === undefined || !Number.isFinite(seconds) || seconds < 0) return '[--:--]';

    const totalSeconds = Math.floor(seconds);
    const minutes = Math.floor(totalSeconds / 60);
    const secs = totalSeconds % 60;

    return `[${minutes.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}]`;
}

// Memoized transcript segment component
const TranscriptSegment = memo(function TranscriptSegment({
    id,
    timestamp,
    text,
    confidence,
    speaker,
    showSpeakerLabels,
    isStreaming,
    showConfidence,
    onSpeakerChange,
    onApplySpeakerToMatching,
    speakerCatalog,
    sourceAttributionEnabled,
    attendeeNames,
    isSpeakerHighlighted,
    isActive,
    onSeekToTime,
    onEdit,
    onBookmark,
    originalText,
    highlight,
    isFindCurrent = false,
}: {
    highlight?: string;
    isFindCurrent?: boolean;
    onEdit?: (id: string) => void;
    onBookmark?: (seconds: number, anchor: HTMLButtonElement) => void;
    originalText?: string | null;
    id: string;
    timestamp: number;
    text: string;
    confidence?: number | null;
    speaker?: string;
    showSpeakerLabels: boolean;
    isStreaming: boolean;
    showConfidence: boolean;
    onSpeakerChange?: (segmentId: string, speaker: string | null) => Promise<void> | void;
    onApplySpeakerToMatching?: (fromSpeaker: string | null | undefined, speaker: string | null) => Promise<number> | number | void;
    speakerCatalog: readonly SpeakerDetails[];
    sourceAttributionEnabled: boolean;
    attendeeNames?: readonly string[];
    isSpeakerHighlighted: boolean;
    /** Whether playback is currently inside this segment. Computed by the
     * parent so a clock tick only re-renders the rows whose state flips. */
    isActive: boolean;
    onSeekToTime?: (seconds: number) => void;
}) {
    const displayText = formatTranscriptDisplayText(text);
    const content = highlight && text.trim()
        ? splitForHighlight(displayText, highlight).map((part, index) => part.match
            ? <mark key={index} className="rounded-sm bg-warning px-0.5 text-foreground">{part.text}</mark>
            : part.text)
        : displayText;
    const currentSpeaker = showSpeakerLabels ? speakerLabel(speaker) : null;
    const hasTimestamp = Number.isFinite(timestamp) && timestamp >= 0;
    const canSeek = Boolean(onSeekToTime && hasTimestamp);
    const canBookmark = Boolean(onBookmark && hasTimestamp);

    const seekToSegment = () => {
        if (canSeek) {
            onSeekToTime?.(timestamp);
        }
    };

    return (
        <div
            id={`segment-${id}`}
            data-speaker-highlighted={isSpeakerHighlighted || undefined}
            className={`group/segment relative mb-3 rounded-[4px] px-1 py-0.5 transition-colors motion-reduce:transition-none ${isSpeakerHighlighted ? "before:absolute before:inset-y-0 before:left-0 before:w-0.5 before:rounded-sm before:bg-primary" : ""} ${isFindCurrent ? "bg-primary/10 ring-2 ring-ring" : isActive ? "bg-primary/10 ring-1 ring-primary/30" : isSpeakerHighlighted ? "bg-accent/55" : ""}`}
        >
            <div className="flex items-start gap-3">
                <Tooltip>
                    <TooltipTrigger asChild>
                        <Button
                            variant="ghost"
                            type="button"
                            disabled={!canSeek}
                            onClick={seekToSegment}
                            aria-label={`Play recording from ${formatRecordingTime(timestamp)}`}
                            className="h-auto justify-start gap-0 p-0 whitespace-normal font-normal hover:bg-transparent hover:text-muted-foreground disabled:pointer-events-auto disabled:opacity-100 flex min-h-8 min-w-[3.5rem] flex-shrink-0 items-center rounded font-mono text-[11px] tabular-nums text-muted-foreground enabled:hover:bg-muted enabled:hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-default"
                        >
                            {formatRecordingTime(timestamp)}
                        </Button>
                    </TooltipTrigger>
                    <TooltipContent>
                        {showConfidence && (
                            <ConfidenceIndicator confidence={confidence} showIndicator={showConfidence} />
                        )}
                    </TooltipContent>
                </Tooltip>
                <div className="min-w-0 flex-1">
                    {showSpeakerLabels && onSpeakerChange ? (
                        <SpeakerLabelPopover segmentId={id} timestamp={formatRecordingTime(timestamp).slice(1, -1)}
                            speaker={speaker} catalog={speakerCatalog} sourceAttributionEnabled={sourceAttributionEnabled} attendeeNames={attendeeNames}
                            onSpeakerChange={onSpeakerChange} onRename={onApplySpeakerToMatching} />
                    ) : showSpeakerLabels && currentSpeaker && (
                        <span className="mb-0.5 inline-flex items-center gap-1.5 rounded bg-muted px-1.5 py-0.5 text-[10px] font-semibold text-foreground">
                            <SpeakerDot color={getSpeakerColor(speakerCatalog, currentSpeaker)} />{currentSpeaker}
                        </span>
                    )}
                    {isStreaming ? (
                        <div className="bg-muted border border-border rounded-lg px-3 py-2">
                            <p className="font-reading text-[15px] leading-[1.5] text-foreground/90">{content}</p>
                        </div>
                    ) : (
                        <p className="font-reading text-[15px] leading-[1.5] text-foreground/90">{content}</p>
                    )}
                </div>
                {(onEdit || canBookmark) && <div className="flex shrink-0 items-center gap-1">
                    {onEdit && originalText != null && originalText !== text && <span className="text-xs text-primary">Edited</span>}
                    {canBookmark && <Tooltip><TooltipTrigger asChild>
                      <Button variant="ghost" size="icon" type="button" onClick={event => onBookmark?.(timestamp, event.currentTarget)} className="h-7 w-7 opacity-0 group-hover/segment:opacity-100 group-focus-within/segment:opacity-100 focus-visible:opacity-100" aria-label={`Add bookmark at ${formatRecordingTime(timestamp).slice(1, -1)}`}><BookmarkPlus className="h-3.5 w-3.5" /></Button>
                    </TooltipTrigger><TooltipContent>Add bookmark</TooltipContent></Tooltip>}
                    {onEdit && <Tooltip><TooltipTrigger asChild>
                      <Button variant="ghost" size="icon" type="button" onClick={() => onEdit(id)} className="h-7 w-7 opacity-0 group-hover/segment:opacity-100 group-focus-within/segment:opacity-100 focus-visible:opacity-100" aria-label="Edit segment"><Pencil className="h-3.5 w-3.5" /></Button>
                    </TooltipTrigger><TooltipContent>Edit segment</TooltipContent></Tooltip>}
                </div>}
            </div>
        </div>
    );
});

export const VirtualizedTranscriptView: React.FC<VirtualizedTranscriptViewProps> = ({
    onEditSegment,
    onBookmarkSegment,
    speakerSegments,
    sourceAttributionEnabled = false,
    attendeeNames,
    highlightedSpeaker,
    focusedSource,
    segments,
    emptyState,
    header,
    beforeTranscript,
    contentClassName,
    isRecording = false,
    isPaused = false,
    isProcessing = false,
    isStopping = false,
    enableStreaming = false,
    showConfidence = true,
    disableAutoScroll = false,
    showSpeakerLabels = true,
    activeTime: initialTime,
    playbackClock,
    onSeekToTime,
    followPlayback = false,
    onUserScroll,
    findQuery,
    activeFindId = null,
    findRequest = 0,
    hasMore = false,
    isLoadingMore = false,
    totalCount = 0,
    loadedCount = 0,
    onLoadMore,
    onSpeakerChange,
    onApplySpeakerToMatching,
}) => {
    const activeTime = usePlaybackTime(playbackClock) ?? initialTime;
    const scrollRef = useRef<HTMLDivElement>(null);
    const headerRef = useRef<HTMLDivElement>(null);
    const leadingRef = useRef<HTMLDivElement>(null);
    const contentRef = useRef<HTMLDivElement>(null);
    const [scrollMargin, setScrollMargin] = useState(0);
    const useVirtualization = segments.length >= VIRTUALIZATION_THRESHOLD;
    // Ref for infinite scroll trigger element
    const loadMoreTriggerRef = useRef<HTMLDivElement>(null);

    // Force re-render without flushSync (avoids React warning)
    const [, rerender] = useReducer((x: number) => x + 1, 0);

    // Setup virtualizer for efficient rendering of large lists
    const getItemKey = useCallback((index: number) => segments[index].id, [segments]);
    const virtualizer = useVirtualizer({
        count: segments.length,
        getItemKey,
        getScrollElement: () => scrollRef.current,
        scrollMargin,
        estimateSize: () => 60, // Estimated height per segment
        overscan: 10, // Render extra items above/below viewport
        onChange: () => {
            startTransition(() => {
                rerender();
            });
        },
    });

    useEffect(() => {
        const content = contentRef.current;
        if (!useVirtualization || !content) return;
        // The virtual rows start below the controls and any assistance content
        // in the same scroll area. Use their current offset, including on resize.
        const updateMargin = () => setScrollMargin(content.offsetTop);
        updateMargin();
        const observer = new ResizeObserver(updateMargin);
        if (headerRef.current) observer.observe(headerRef.current);
        if (leadingRef.current) observer.observe(leadingRef.current);
        return () => observer.disconnect();
    }, [useVirtualization]);

    const scrolledSource = useRef<number | undefined>(undefined);
    useEffect(() => {
        if (!focusedSource || scrolledSource.current === focusedSource.request) return;
        const index = segments.findIndex(segment => segment.id === focusedSource.id);
        if (index < 0) return;
        const timer = setTimeout(() => {
            if (segments.length >= VIRTUALIZATION_THRESHOLD) virtualizer.scrollToIndex(index, { align: 'center' });
            else scrollRef.current?.querySelectorAll('[data-segment-id]')[index]?.scrollIntoView({ block: 'nearest' });
            scrolledSource.current = focusedSource.request;
        }, 60);
        return () => clearTimeout(timer);
    }, [focusedSource, segments, virtualizer]);

    const segmentsRef = useRef(segments);
    segmentsRef.current = segments;
    const scrollToSegment = (id: string, align: 'auto' | 'center'): boolean => {
        const index = segmentsRef.current.findIndex(segment => segment.id === id);
        if (index < 0) return false;
        if (segmentsRef.current.length >= VIRTUALIZATION_THRESHOLD) virtualizer.scrollToIndex(index, { align });
        else scrollRef.current?.querySelectorAll('[data-segment-id]')[index]?.scrollIntoView({ block: align === 'center' ? 'center' : 'nearest' });
        return true;
    };

    const scrolledSpeaker = useRef<string | null | undefined>(undefined);
    useEffect(() => {
        if (highlightedSpeaker === undefined) { scrolledSpeaker.current = undefined; return; }
        if (scrolledSpeaker.current === highlightedSpeaker) return;
        const first = segments.find(segment => speakerLabel(segment.speaker) === highlightedSpeaker);
        if (first && scrollToSegment(first.id, 'center')) scrolledSpeaker.current = highlightedSpeaker;
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [highlightedSpeaker, segments]);

    // Find in transcript: bring the current match into view once per request,
    // waiting for its page to load if it is not loaded yet.
    const scrolledFindRequest = useRef(-1);
    useEffect(() => {
        if (!activeFindId || scrolledFindRequest.current === findRequest) return;
        const timer = setTimeout(() => { if (scrollToSegment(activeFindId, 'center')) scrolledFindRequest.current = findRequest; }, 60);
        return () => clearTimeout(timer);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [activeFindId, findRequest, segments]);

    // Reading ahead by hand ends follow-along instead of fighting the reader.
    useEffect(() => {
        const element = scrollRef.current;
        if (!element || !followPlayback || !onUserScroll) return;
        const stop = () => onUserScroll();
        const stopOnScrollbar = (event: MouseEvent) => { if (event.target === element) onUserScroll(); };
        element.addEventListener('wheel', stop, { passive: true });
        element.addEventListener('touchmove', stop, { passive: true });
        element.addEventListener('mousedown', stopOnScrollbar);
        return () => {
            element.removeEventListener('wheel', stop);
            element.removeEventListener('touchmove', stop);
            element.removeEventListener('mousedown', stopOnScrollbar);
        };
    }, [followPlayback, onUserScroll]);

    const liveTranscript = isRecording && !disableAutoScroll;
    const liveFollowEnabled = liveTranscript && !isStopping && !isProcessing;
    const { autoScroll, scrollToBottom } = useAutoScroll({
        scrollRef,
        contentRef,
        segmentCount: segments.length,
        enabled: liveFollowEnabled,
        isPaused,
    });

    // Streaming text effect hook (typewriter animation for new transcripts)
    const { streamingSegmentId, getDisplayText } = useTranscriptStreaming(
        segments,
        isRecording,
        enableStreaming
    );

    // Infinite scroll: IntersectionObserver to trigger loading more
    useEffect(() => {
        if (!onLoadMore || !hasMore || isLoadingMore || isRecording || segments.length === 0) {
            return;
        }

        const triggerElement = loadMoreTriggerRef.current;
        if (!triggerElement) return;

        const observer = new IntersectionObserver(
            (entries) => {
                if (entries[0].isIntersecting && hasMore && !isLoadingMore) {
                    onLoadMore();
                }
            },
            {
                root: null,
                rootMargin: '100px',
                threshold: 0,
            }
        );

        observer.observe(triggerElement);

        return () => observer.disconnect();
    }, [hasMore, isLoadingMore, onLoadMore, isRecording, segments.length]);

    // Scroll-based fallback for fast scrolling
    useEffect(() => {
        if (!onLoadMore || !hasMore || isLoadingMore || isRecording) return;

        const scrollElement = scrollRef.current;
        if (!scrollElement) return;

        let ticking = false;

        const handleScroll = () => {
            if (ticking || isLoadingMore || !hasMore) return;

            ticking = true;
            requestAnimationFrame(() => {
                const { scrollTop, scrollHeight, clientHeight } = scrollElement;
                const scrollBottom = scrollHeight - scrollTop - clientHeight;

                // Trigger load when within 200px of bottom
                if (scrollBottom < 200 && hasMore && !isLoadingMore) {
                    onLoadMore();
                }
                ticking = false;
            });
        };

        scrollElement.addEventListener('scroll', handleScroll, { passive: true });
        return () => scrollElement.removeEventListener('scroll', handleScroll);
    }, [onLoadMore, hasMore, isLoadingMore, isRecording]);

    const speakerCatalog = useMemo(() => buildSpeakerCatalog(speakerSegments?.length ? speakerSegments : segments), [speakerSegments, segments]);

    // Resolve the playback position to a single active segment id, so clock
    // ticks re-render at most the two rows whose highlight state changes
    // instead of every visible (memo-broken) row.
    const hasPlayback = activeTime !== undefined;
    const playbackRanges = useMemo(() => hasPlayback ? indexPlaybackRanges(segments) : [], [hasPlayback, segments]);
    const activeSegmentId = activePlaybackSegment(playbackRanges, activeTime);

    // Follow playback: scroll when the spoken segment changes, never on every clock tick.
    const followedSegment = useRef<string | null>(null);
    useEffect(() => {
        if (!followPlayback) { followedSegment.current = null; return; }
        if (!activeSegmentId || followedSegment.current === activeSegmentId) return;
        if (scrollToSegment(activeSegmentId, 'auto')) followedSegment.current = activeSegmentId;
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [followPlayback, activeSegmentId]);

    return (
        <div className="relative flex h-full min-h-0 flex-col">
            <div
                ref={scrollRef}
                role="region"
                aria-label="Transcript"
                tabIndex={liveTranscript ? 0 : undefined}
                className={`relative min-h-0 flex-1 overflow-y-auto px-4 pt-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring ${liveTranscript ? 'pb-32' : 'pb-2'}`}
            >
            <div ref={headerRef} className="sticky top-0 z-10 bg-card">
                {header}
                <AnimatePresence>
                    {isRecording && (
                        <div className={contentClassName}>
                            <RecordingStatusBar isPaused={isPaused} />
                        </div>
                    )}
                </AnimatePresence>
            </div>
            <div className={contentClassName}>
            <div ref={leadingRef} className={isRecording ? 'pb-2' : undefined}>{beforeTranscript}</div>
            <div ref={contentRef}>
            {segments.length === 0 ? (
                // Empty state
                <motion.div
                    initial={{ opacity: 0 }}
                    animate={{ opacity: 1 }}
                    className="text-center text-muted-foreground mt-8"
                >
                    {isRecording ? (
                        <>
                            <div className="flex items-center justify-center mb-3">
                                <div className={`w-3 h-3 rounded-full ${isPaused ? 'bg-warning-foreground' : 'bg-primary animate-pulse'}`}></div>
                            </div>
                            <p className="text-sm text-muted-foreground">
                                {isPaused ? 'Recording paused' : 'Listening for speech...'}
                            </p>
                            <p className="text-xs mt-1 text-muted-foreground">
                                {isPaused ? 'Click resume to continue recording' : 'Speak to see live transcription'}
                            </p>
                        </>
                    ) : emptyState || (
                        <>
                            <p className="text-lg font-semibold">Welcome to ClawScribe</p>
                            <p className="text-xs mt-1">Start recording to see live transcription</p>
                        </>
                    )}
                </motion.div>
            ) : useVirtualization ? (
                // Virtualized rendering for large lists
                <>
                    <div
                        style={{
                            height: virtualizer.getTotalSize(),
                            width: "100%",
                            position: "relative",
                        }}
                    >
                        {virtualizer.getVirtualItems().map((virtualRow) => {
                            const segment = segments[virtualRow.index];
                            const isStreaming = streamingSegmentId === segment.id;

                            return (
                                <div
                                    key={segment.id}
                                    data-segment-id={segment.id}
                                    data-index={virtualRow.index}
                                    ref={virtualizer.measureElement}
                                    className="flow-root pb-1"
                                    style={{
                                        position: "absolute",
                                        top: 0,
                                        left: 0,
                                        width: "100%",
                                        transform: `translateY(${virtualRow.start - scrollMargin}px)`,
                                    }}
                                >
                                    <TranscriptSegment
                                        id={segment.id}
                                        timestamp={segment.timestamp}
                                        text={getDisplayText(segment)}
                                        originalText={segment.original_text}
                                        onEdit={onEditSegment}
                                        onBookmark={onBookmarkSegment}
                                        confidence={segment.confidence}
                                        speaker={segment.speaker}
                                        showSpeakerLabels={showSpeakerLabels}
                                        isStreaming={isStreaming}
                                        showConfidence={showConfidence}
                                        onSpeakerChange={showSpeakerLabels ? onSpeakerChange : undefined}
                                        onApplySpeakerToMatching={showSpeakerLabels ? onApplySpeakerToMatching : undefined}
                                        speakerCatalog={speakerCatalog}
                                        sourceAttributionEnabled={sourceAttributionEnabled}
                                        attendeeNames={attendeeNames}
                                        isSpeakerHighlighted={highlightedSpeaker !== undefined && speakerLabel(segment.speaker) === highlightedSpeaker}
                                        isActive={segment.id === activeSegmentId || segment.id === focusedSource?.id}
                                        onSeekToTime={onSeekToTime}
                                        highlight={findQuery}
                                        isFindCurrent={segment.id === activeFindId}
                                    />
                                </div>
                            );
                        })}
                    </div>

                    {/* Infinite scroll trigger and loading indicator */}
                    {(hasMore || isLoadingMore) && !isRecording && segments.length > 0 && (
                        <div ref={loadMoreTriggerRef} className="flex justify-center items-center py-4 mt-2">
                            {isLoadingMore ? (
                                <div className="flex items-center gap-2 text-muted-foreground">
                                    <div className="w-4 h-4 border-2 border-border border-t-muted-foreground rounded-full animate-spin" />
                                    <span className="text-sm">Loading more...</span>
                                </div>
                            ) : hasMore && totalCount > 0 ? (
                                <span className="text-sm text-muted-foreground">
                                    Showing {loadedCount} of {totalCount} segments
                                </span>
                            ) : null}
                        </div>
                    )}

                    {/* Listening indicator when recording */}
                    {!isStopping && isRecording && !isPaused && !isProcessing && segments.length > 0 && (
                        <motion.div
                            initial={{ opacity: 0 }}
                            animate={{ opacity: 1 }}
                            exit={{ opacity: 0 }}
                            className="flex items-center gap-2 mt-4 text-muted-foreground"
                        >
                            <div className="w-2 h-2 bg-primary rounded-full animate-pulse"></div>
                            <span className="text-sm">Listening...</span>
                        </motion.div>
                    )}
                </>
            ) : (
                // Simple rendering for small lists (better animations)
                <>
                    <div>
                        {segments.map((segment) => {
                            const isStreaming = streamingSegmentId === segment.id;

                            return (
                                <motion.div
                                    key={segment.id}
                                    data-segment-id={segment.id}
                                    className="flow-root pb-1"
                                    initial={{ opacity: 0, y: 5 }}
                                    animate={{ opacity: 1, y: 0 }}
                                    transition={{ duration: 0.15 }}
                                >
                                    <TranscriptSegment
                                        id={segment.id}
                                        timestamp={segment.timestamp}
                                        text={getDisplayText(segment)}
                                        originalText={segment.original_text}
                                        onEdit={onEditSegment}
                                        onBookmark={onBookmarkSegment}
                                        confidence={segment.confidence}
                                        speaker={segment.speaker}
                                        showSpeakerLabels={showSpeakerLabels}
                                        isStreaming={isStreaming}
                                        showConfidence={showConfidence}
                                        onSpeakerChange={showSpeakerLabels ? onSpeakerChange : undefined}
                                        onApplySpeakerToMatching={showSpeakerLabels ? onApplySpeakerToMatching : undefined}
                                        speakerCatalog={speakerCatalog}
                                        sourceAttributionEnabled={sourceAttributionEnabled}
                                        attendeeNames={attendeeNames}
                                        isSpeakerHighlighted={highlightedSpeaker !== undefined && speakerLabel(segment.speaker) === highlightedSpeaker}
                                        isActive={segment.id === activeSegmentId || segment.id === focusedSource?.id}
                                        onSeekToTime={onSeekToTime}
                                        highlight={findQuery}
                                        isFindCurrent={segment.id === activeFindId}
                                    />
                                </motion.div>
                            );
                        })}
                    </div>

                    {/* Infinite scroll trigger (for small lists that grow) */}
                    {(hasMore || isLoadingMore) && !isRecording && segments.length > 0 && (
                        <div ref={loadMoreTriggerRef} className="flex justify-center items-center py-4 mt-2">
                            {isLoadingMore ? (
                                <div className="flex items-center gap-2 text-muted-foreground">
                                    <div className="w-4 h-4 border-2 border-border border-t-muted-foreground rounded-full animate-spin" />
                                    <span className="text-sm">Loading more...</span>
                                </div>
                            ) : hasMore && totalCount > 0 ? (
                                <span className="text-sm text-muted-foreground">
                                    Showing {loadedCount} of {totalCount} segments
                                </span>
                            ) : null}
                        </div>
                    )}

                    {/* Listening indicator when recording */}
                    {!isStopping && isRecording && !isPaused && !isProcessing && segments.length > 0 && (
                        <motion.div
                            initial={{ opacity: 0 }}
                            animate={{ opacity: 1 }}
                            exit={{ opacity: 0 }}
                            className="flex items-center gap-2 mt-4 text-muted-foreground"
                        >
                            <div className="w-2 h-2 bg-primary rounded-full animate-pulse"></div>
                            <span className="text-sm">Listening...</span>
                        </motion.div>
                    )}
                </>
            )}
            </div>
            </div>
            </div>
            {liveFollowEnabled && !autoScroll && segments.length > 0 && (
                <div className="pointer-events-none absolute inset-x-4 bottom-24 z-20 flex justify-end">
                    <Button
                        variant="ghost"
                        type="button"
                        onClick={scrollToBottom}
                        className="h-auto gap-0 py-0 whitespace-normal hover:text-foreground pointer-events-auto min-h-9 rounded-full border border-border bg-card px-4 text-sm font-medium text-foreground shadow-sm hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                    >
                        Jump to live transcript
                    </Button>
                </div>
            )}
        </div>
    );
};
