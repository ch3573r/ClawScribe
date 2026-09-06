'use client';

import { useRef, useReducer, startTransition, useEffect, useState, memo, useMemo, useId } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useAutoScroll } from "@/hooks/useAutoScroll";
import { useTranscriptStreaming } from "@/hooks/useTranscriptStreaming";
import { formatTranscriptDisplayText } from "@/lib/transcriptDisplay";
import { toast } from "sonner";
import { ConfidenceIndicator } from "./ConfidenceIndicator";
import { Tooltip, TooltipContent, TooltipTrigger } from "./ui/tooltip";
import {
    DropdownMenu,
    DropdownMenuContent,
    DropdownMenuItem,
    DropdownMenuLabel,
    DropdownMenuSeparator,
    DropdownMenuSub,
    DropdownMenuSubContent,
    DropdownMenuSubTrigger,
    DropdownMenuTrigger,
} from "./ui/dropdown-menu";
import { RecordingStatusBar } from "./RecordingStatusBar";
import { motion, AnimatePresence } from "framer-motion";
import { TranscriptSegmentData } from "@/types";

export interface VirtualizedTranscriptViewProps {
    /** Transcript segments to display */
    segments: TranscriptSegmentData[];
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
    /** Seek recording playback to a transcript timestamp. */
    onSeekToTime?: (seconds: number) => void;

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
const SPEAKER_PRESET_LABELS = ["Me", "Participants"];

// Helper function to format seconds as recording-relative time [MM:SS]
function formatRecordingTime(seconds: number | undefined): string {
    if (seconds === undefined || !Number.isFinite(seconds) || seconds < 0) return '[--:--]';

    const totalSeconds = Math.floor(seconds);
    const minutes = Math.floor(totalSeconds / 60);
    const secs = totalSeconds % 60;

    return `[${minutes.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}]`;
}

function normalizeSpeakerOption(speaker: string | null | undefined): string | null {
    const label = speaker?.trim().replace(/\s+/g, " ");
    return label || null;
}

function collectSpeakerOptions(segments: TranscriptSegmentData[]): string[] {
    const seen = new Set<string>();
    const options: string[] = [];

    for (const segment of segments) {
        const label = normalizeSpeakerOption(segment.speaker);
        if (label && !seen.has(label)) {
            seen.add(label);
            options.push(label);
        }
    }

    return options;
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
    speakerOptions,
    isActive,
    onSeekToTime,
}: {
    id: string;
    timestamp: number;
    text: string;
    confidence?: number;
    speaker?: string;
    showSpeakerLabels: boolean;
    isStreaming: boolean;
    showConfidence: boolean;
    onSpeakerChange?: (segmentId: string, speaker: string | null) => Promise<void> | void;
    onApplySpeakerToMatching?: (fromSpeaker: string | null | undefined, speaker: string | null) => Promise<number> | number | void;
    speakerOptions: string[];
    /** Whether playback is currently inside this segment. Computed by the
     * parent so a clock tick only re-renders the rows whose state flips. */
    isActive: boolean;
    onSeekToTime?: (seconds: number) => void;
}) {
    const displayText = formatTranscriptDisplayText(text);
    // "Me" = your microphone, "Participants" = system audio. Color-code so the
    // two sides of the conversation are scannable.
    const currentSpeaker = showSpeakerLabels ? normalizeSpeakerOption(speaker) : null;
    const isMe = currentSpeaker === "Me";
    const [customSpeaker, setCustomSpeaker] = useState("");
    const [isSaving, setIsSaving] = useState(false);
    const savingRef = useRef(false);
    const speakerInputId = useId();
    const canReplaceMatching = Boolean(currentSpeaker && onApplySpeakerToMatching);
    const presetOptions = SPEAKER_PRESET_LABELS.filter((label) => label !== currentSpeaker);
    const existingSpeakerOptions = speakerOptions.filter(
        (label) => label !== currentSpeaker && !SPEAKER_PRESET_LABELS.includes(label)
    );
    const replacementVerb = canReplaceMatching ? "Replace" : "Set";
    const canSeek = Boolean(onSeekToTime && Number.isFinite(timestamp) && timestamp >= 0);

    const seekToSegment = () => {
        if (canSeek) {
            onSeekToTime?.(timestamp);
        }
    };

    const persistSpeakerChange = async (operation: () => Promise<unknown> | unknown): Promise<boolean> => {
        if (savingRef.current) return false;
        savingRef.current = true;
        setIsSaving(true);
        try {
            await operation();
            return true;
        } catch {
            toast.error("Could not save the speaker label. Please try again.");
            return false;
        } finally {
            savingRef.current = false;
            setIsSaving(false);
        }
    };

    const saveSpeaker = (nextSpeaker: string | null): Promise<boolean> => {
        if (!onSpeakerChange) return Promise.resolve(false);
        return persistSpeakerChange(() => onSpeakerChange(id, nextSpeaker));
    };

    const replaceSpeaker = (nextSpeaker: string | null): Promise<boolean> => {
        if (canReplaceMatching && onApplySpeakerToMatching) {
            return persistSpeakerChange(() => onApplySpeakerToMatching(currentSpeaker, nextSpeaker));
        }
        return saveSpeaker(nextSpeaker);
    };

    const saveCustomSpeaker = async () => {
        const nextSpeaker = customSpeaker.trim().replace(/\s+/g, " ");
        if (!nextSpeaker) return;
        if (await replaceSpeaker(nextSpeaker)) setCustomSpeaker("");
    };

    const speakerClass = isMe
        ? "bg-primary/10 text-foreground hover:bg-primary/15"
        : currentSpeaker
            ? "bg-muted text-muted-foreground hover:bg-muted/80"
            : "border border-dashed border-border bg-transparent text-muted-foreground hover:bg-muted";

    return (
        <div
            id={`segment-${id}`}
            className={`mb-4 rounded-[4px] px-1 py-0.5 transition-colors motion-reduce:transition-none ${isActive ? "bg-accent/10 ring-1 ring-accent/30" : ""}`}
        >
            <div className="flex items-start gap-3">
                <Tooltip>
                    <TooltipTrigger asChild>
                        <button
                            type="button"
                            disabled={!canSeek}
                            onClick={seekToSegment}
                            aria-label={`Play recording from ${formatRecordingTime(timestamp)}`}
                            className="flex min-h-8 min-w-[3.5rem] flex-shrink-0 items-center rounded font-mono text-[11px] tabular-nums text-muted-foreground enabled:hover:bg-muted enabled:hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-default"
                        >
                            {formatRecordingTime(timestamp)}
                        </button>
                    </TooltipTrigger>
                    <TooltipContent>
                        {confidence !== undefined && showConfidence && (
                            <ConfidenceIndicator confidence={confidence} showIndicator={showConfidence} />
                        )}
                    </TooltipContent>
                </Tooltip>
                <div className="min-w-0 flex-1">
                    {showSpeakerLabels && onSpeakerChange ? (
                        <DropdownMenu>
                            <DropdownMenuTrigger asChild>
                                <button
                                    type="button"
                                    disabled={isSaving}
                                    onClick={(event) => event.stopPropagation()}
                                    onKeyDown={(event) => event.stopPropagation()}
                                    className={`mb-0.5 inline-flex max-w-[11rem] items-center rounded px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide transition disabled:opacity-60 ${speakerClass}`}
                                >
                                    <span className="truncate">{currentSpeaker ?? "Label"}</span>
                                </button>
                            </DropdownMenuTrigger>
                            <DropdownMenuContent align="start" className="w-64">
                                <DropdownMenuLabel className="min-w-0">
                                    <span className="block truncate">
                                        {canReplaceMatching ? `Replace ${currentSpeaker}` : "Speaker label"}
                                    </span>
                                </DropdownMenuLabel>
                                {presetOptions.map((label) => (
                                    <DropdownMenuItem key={label} onSelect={() => void replaceSpeaker(label)}>
                                        {canReplaceMatching ? `Replace with ${label}` : label}
                                    </DropdownMenuItem>
                                ))}
                                <DropdownMenuItem onSelect={() => void replaceSpeaker(null)}>
                                    {canReplaceMatching ? "Clear matching labels" : "Clear label"}
                                </DropdownMenuItem>
                                {existingSpeakerOptions.length > 0 && (
                                    <>
                                        <DropdownMenuSeparator />
                                        <DropdownMenuLabel className="px-2 py-1 text-xs font-medium text-muted-foreground">
                                            Existing labels
                                        </DropdownMenuLabel>
                                        {existingSpeakerOptions.map((label) => (
                                            <DropdownMenuItem
                                                key={label}
                                                onSelect={() => void replaceSpeaker(label)}
                                                className="min-w-0"
                                            >
                                                <span className="truncate">
                                                    {canReplaceMatching ? `Replace with ${label}` : label}
                                                </span>
                                            </DropdownMenuItem>
                                        ))}
                                    </>
                                )}
                                <DropdownMenuSeparator />
                                <div
                                    className="space-y-1.5 px-2 py-1.5"
                                    onClick={(e) => e.stopPropagation()}
                                    onKeyDown={(e) => e.stopPropagation()}
                                >
                                    <label htmlFor={speakerInputId} className="text-xs font-medium text-muted-foreground">
                                        {canReplaceMatching ? "Custom replacement" : "Custom label"}
                                    </label>
                                    <div className="flex gap-1.5">
                                        <input
                                            id={speakerInputId}
                                            disabled={isSaving}
                                            value={customSpeaker}
                                            onChange={(e) => setCustomSpeaker(e.target.value)}
                                            onKeyDown={(e) => {
                                                if (e.key === "Enter") {
                                                    e.preventDefault();
                                                    void saveCustomSpeaker();
                                                }
                                            }}
                                            maxLength={64}
                                            placeholder="Speaker name"
                                            className="min-w-0 flex-1 rounded border border-border bg-background px-2 py-1 text-xs outline-none focus:border-primary"
                                        />
                                        <button
                                            type="button"
                                            onClick={() => void saveCustomSpeaker()}
                                            disabled={isSaving || !customSpeaker.trim()}
                                            className="rounded border border-border px-2 py-1 text-xs font-medium text-foreground hover:bg-muted disabled:opacity-50"
                                        >
                                            {replacementVerb}
                                        </button>
                                    </div>
                                </div>
                                {canReplaceMatching && (
                                    <>
                                        <DropdownMenuSeparator />
                                        <DropdownMenuSub>
                                            <DropdownMenuSubTrigger className="min-w-0">
                                                <span className="truncate">Only this segment</span>
                                            </DropdownMenuSubTrigger>
                                            <DropdownMenuSubContent className="w-56">
                                                {presetOptions.map((label) => (
                                                    <DropdownMenuItem key={label} onSelect={() => void saveSpeaker(label)}>
                                                        Set to {label}
                                                    </DropdownMenuItem>
                                                ))}
                                                {existingSpeakerOptions.length > 0 && (
                                                    <>
                                                        <DropdownMenuSeparator />
                                                        {existingSpeakerOptions.map((label) => (
                                                            <DropdownMenuItem
                                                                key={label}
                                                                onSelect={() => void saveSpeaker(label)}
                                                                className="min-w-0"
                                                            >
                                                                <span className="truncate">Set to {label}</span>
                                                            </DropdownMenuItem>
                                                        ))}
                                                    </>
                                                )}
                                                <DropdownMenuSeparator />
                                                <DropdownMenuItem onSelect={() => void saveSpeaker(null)}>
                                                    Clear this label
                                                </DropdownMenuItem>
                                            </DropdownMenuSubContent>
                                        </DropdownMenuSub>
                                    </>
                                )}
                            </DropdownMenuContent>
                        </DropdownMenu>
                    ) : showSpeakerLabels && currentSpeaker && (
                        <span
                            className={`mb-0.5 inline-block rounded px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide ${speakerClass}`}
                        >
                            {currentSpeaker}
                        </span>
                    )}
                    {isStreaming ? (
                        <div className="bg-muted border border-border rounded-lg px-3 py-2">
                            <p className="text-[15px] leading-7 text-foreground/90">{displayText}</p>
                        </div>
                    ) : (
                        <p className="text-[15px] leading-7 text-foreground/90">{displayText}</p>
                    )}
                </div>
            </div>
        </div>
    );
});

export const VirtualizedTranscriptView: React.FC<VirtualizedTranscriptViewProps> = ({
    segments,
    isRecording = false,
    isPaused = false,
    isProcessing = false,
    isStopping = false,
    enableStreaming = false,
    showConfidence = true,
    disableAutoScroll = false,
    showSpeakerLabels = true,
    activeTime,
    onSeekToTime,
    hasMore = false,
    isLoadingMore = false,
    totalCount = 0,
    loadedCount = 0,
    onLoadMore,
    onSpeakerChange,
    onApplySpeakerToMatching,
}) => {
    // Create scroll ref first - shared between virtualizer and auto-scroll hook
    const scrollRef = useRef<HTMLDivElement>(null);
    // Ref for infinite scroll trigger element
    const loadMoreTriggerRef = useRef<HTMLDivElement>(null);

    // Force re-render without flushSync (avoids React warning)
    const [, rerender] = useReducer((x: number) => x + 1, 0);

    // Setup virtualizer for efficient rendering of large lists
    const virtualizer = useVirtualizer({
        count: segments.length,
        getScrollElement: () => scrollRef.current,
        estimateSize: () => 60, // Estimated height per segment
        overscan: 10, // Render extra items above/below viewport
        onChange: () => {
            startTransition(() => {
                rerender();
            });
        },
    });

    // Custom hook for auto-scrolling (supports both virtualized and non-virtualized)
    const { autoScroll, scrollToBottom } = useAutoScroll({
        scrollRef,
        segments,
        isRecording,
        isPaused,
        virtualizer,
        virtualizationThreshold: VIRTUALIZATION_THRESHOLD,
        disableAutoScroll,
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

    // Use simple rendering for small lists, virtualization for large lists
    const useVirtualization = segments.length >= VIRTUALIZATION_THRESHOLD;
    const speakerOptions = useMemo(() => collectSpeakerOptions(segments), [segments]);

    // Resolve the playback position to a single active segment id, so clock
    // ticks re-render at most the two rows whose highlight state changes
    // instead of every visible (memo-broken) row.
    const activeSegmentId = useMemo(() => {
        if (activeTime === undefined) return null;
        for (const segment of segments) {
            const end = segment.endTime ?? segment.timestamp + 0.75;
            if (activeTime >= segment.timestamp && activeTime < end) {
                return segment.id;
            }
        }
        return null;
    }, [segments, activeTime]);

    return (
        <div ref={scrollRef} tabIndex={0} role="region" aria-label={isRecording ? "Live transcript" : "Meeting transcript"} className="flex flex-col h-full overflow-y-auto px-4 py-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring">
            {/* Recording Status Bar - Sticky at top, always visible when recording */}
            <AnimatePresence>
                {isRecording && (
                    <div className="sticky top-0 z-10 bg-card pb-2">
                        <RecordingStatusBar isPaused={isPaused} />
                    </div>
                )}
            </AnimatePresence>

            {/* Content - add padding when recording to prevent overlap */}
            <div className={isRecording ? 'pt-2' : ''}>
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
                                <div className={`w-3 h-3 rounded-full ${isPaused ? 'bg-orange-500' : 'bg-primary animate-pulse'}`}></div>
                            </div>
                            <p className="text-sm text-muted-foreground">
                                {isPaused ? 'Recording paused' : 'Listening for speech...'}
                            </p>
                            <p className="text-xs mt-1 text-muted-foreground">
                                {isPaused ? 'Click resume to continue recording' : 'Speak to see live transcription'}
                            </p>
                        </>
                    ) : (
                        <>
                            <p className="text-lg font-semibold">Welcome to ClawScribe!</p>
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
                                    data-index={virtualRow.index}
                                    ref={virtualizer.measureElement}
                                    style={{
                                        position: "absolute",
                                        top: 0,
                                        left: 0,
                                        width: "100%",
                                        transform: `translateY(${virtualRow.start}px)`,
                                    }}
                                >
                                    <TranscriptSegment
                                        id={segment.id}
                                        timestamp={segment.timestamp}
                                        text={getDisplayText(segment)}
                                        confidence={segment.confidence}
                                        speaker={segment.speaker}
                                        showSpeakerLabels={showSpeakerLabels}
                                        isStreaming={isStreaming}
                                        showConfidence={showConfidence}
                                        onSpeakerChange={showSpeakerLabels ? onSpeakerChange : undefined}
                                        onApplySpeakerToMatching={showSpeakerLabels ? onApplySpeakerToMatching : undefined}
                                        speakerOptions={speakerOptions}
                                        isActive={segment.id === activeSegmentId}
                                        onSeekToTime={onSeekToTime}
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
                                    <div className="w-4 h-4 border-2 border-border border-t-gray-600 rounded-full animate-spin" />
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
                    <div className="space-y-1">
                        {segments.map((segment) => {
                            const isStreaming = streamingSegmentId === segment.id;

                            return (
                                <motion.div
                                    key={segment.id}
                                    initial={{ opacity: 0, y: 5 }}
                                    animate={{ opacity: 1, y: 0 }}
                                    transition={{ duration: 0.15 }}
                                >
                                    <TranscriptSegment
                                        id={segment.id}
                                        timestamp={segment.timestamp}
                                        text={getDisplayText(segment)}
                                        confidence={segment.confidence}
                                        speaker={segment.speaker}
                                        showSpeakerLabels={showSpeakerLabels}
                                        isStreaming={isStreaming}
                                        showConfidence={showConfidence}
                                        onSpeakerChange={showSpeakerLabels ? onSpeakerChange : undefined}
                                        onApplySpeakerToMatching={showSpeakerLabels ? onApplySpeakerToMatching : undefined}
                                        speakerOptions={speakerOptions}
                                        isActive={segment.id === activeSegmentId}
                                        onSeekToTime={onSeekToTime}
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
                                    <div className="w-4 h-4 border-2 border-border border-t-gray-600 rounded-full animate-spin" />
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
            {isRecording && !disableAutoScroll && !autoScroll && segments.length > 0 && (
                <div className="sticky bottom-3 z-20 flex justify-end pt-2 pointer-events-none">
                    <button
                        type="button"
                        onClick={scrollToBottom}
                        className="pointer-events-auto min-h-9 rounded-full border border-border bg-card px-4 text-sm font-medium text-foreground shadow-sm hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                    >
                        Jump to live transcript
                    </button>
                </div>
            )}
        </div>
    );
};
