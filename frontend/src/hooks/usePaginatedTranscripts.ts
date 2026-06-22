import { useState, useCallback, useRef, useEffect, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Transcript, MeetingMetadata, PaginatedTranscriptsResponse, TranscriptSegmentData } from "@/types";

const DEFAULT_PAGE_SIZE = 100;

interface UsePaginatedTranscriptsProps {
    meetingId: string | null;
    /** Optional initial timestamp (in seconds) from URL for loading the correct page */
    initialTimestamp?: number;
}

interface UsePaginatedTranscriptsReturn {
    metadata: MeetingMetadata | null;
    segments: TranscriptSegmentData[];
    transcripts: Transcript[];
    isLoading: boolean;
    isLoadingMore: boolean;
    hasMore: boolean;
    totalCount: number;
    loadedCount: number;
    error: string | null;

    // Actions
    loadMore: () => Promise<void>;
    reset: () => void;
    refetch: () => Promise<void>;
    updateSpeaker: (transcriptId: string, speaker: string | null) => Promise<void>;
    applySpeakerToMatching: (fromSpeaker: string | null | undefined, speaker: string | null) => Promise<number>;
}

/**
 * Convert Transcript array to TranscriptSegmentData for virtualized display
 */
function convertTranscriptsToSegments(transcripts: Transcript[]): TranscriptSegmentData[] {
    return transcripts.map(t => ({
        id: t.id,
        timestamp: t.audio_start_time ?? 0,
        endTime: t.audio_end_time,
        text: t.text,
        confidence: t.confidence,
        speaker: t.speaker,
        word_timestamps: t.word_timestamps,
    }));
}

function normalizeSpeaker(speaker: string | null | undefined): string | null {
    const label = speaker?.trim().replace(/\s+/g, " ") ?? "";
    return label ? label.slice(0, 64) : null;
}

export function usePaginatedTranscripts({
    meetingId,
    initialTimestamp,
}: UsePaginatedTranscriptsProps): UsePaginatedTranscriptsReturn {
    const [metadata, setMetadata] = useState<MeetingMetadata | null>(null);
    const [transcripts, setTranscripts] = useState<Transcript[]>([]);
    const [totalCount, setTotalCount] = useState(0);
    const [isLoading, setIsLoading] = useState(true);
    const [isLoadingMore, setIsLoadingMore] = useState(false);
    const [hasMore, setHasMore] = useState(false);
    const [error, setError] = useState<string | null>(null);

    const offsetRef = useRef(0);
    const loadedMeetingIdRef = useRef<string | null>(null);
    const isLoadingRef = useRef(false);
    const lastLoadTimeRef = useRef(0); // Debounce protection

    // Reset state when meeting changes
    const reset = useCallback(() => {
        setMetadata(null);
        setTranscripts([]);
        setTotalCount(0);
        setIsLoading(true);
        setIsLoadingMore(false);
        setHasMore(false);
        setError(null);
        offsetRef.current = 0;
    }, []);

    // Load meeting metadata
    const loadMetadata = useCallback(async (): Promise<MeetingMetadata | null> => {
        if (!meetingId) return null;

        try {
            const data = await invoke<MeetingMetadata>('api_get_meeting_metadata', {
                meetingId,
            });
            setMetadata(data);
            return data;
        } catch (err) {
            console.error('Failed to load meeting metadata:', err);
            setError('Failed to load meeting details');
            return null;
        }
    }, [meetingId]);

    // Load transcripts at specific offset
    const loadTranscriptsAtOffset = useCallback(async (
        offset: number,
        append: boolean = true
    ): Promise<Transcript[]> => {
        if (!meetingId) return [];

        try {
            const response = await invoke<PaginatedTranscriptsResponse>(
                'api_get_meeting_transcripts',
                {
                    meetingId,
                    limit: DEFAULT_PAGE_SIZE,
                    offset,
                }
            );

            const newTranscripts = response.transcripts;

            if (append) {
                setTranscripts(prev => {
                    // Deduplicate by id
                    const existingIds = new Set(prev.map(t => t.id));
                    const uniqueNew = newTranscripts.filter(t => !existingIds.has(t.id));
                    // Sort by audio_start_time
                    return [...prev, ...uniqueNew].sort((a, b) =>
                        (a.audio_start_time ?? 0) - (b.audio_start_time ?? 0)
                    );
                });
            } else {
                setTranscripts(newTranscripts);
            }

            setHasMore(response.has_more);
            setTotalCount(response.total_count);
            offsetRef.current = offset + newTranscripts.length;

            return newTranscripts;
        } catch (err) {
            console.error('Failed to load transcripts:', err);
            setError('Failed to load transcripts');
            return [];
        }
    }, [meetingId]);

    // Load next page with debounce protection
    const loadMore = useCallback(async () => {
        const now = Date.now();
        // Debounce: require at least 100ms between calls
        if (now - lastLoadTimeRef.current < 100) {
            return;
        }

        if (isLoadingRef.current || !hasMore || !meetingId || isLoading) return;

        lastLoadTimeRef.current = now;
        isLoadingRef.current = true;
        setIsLoadingMore(true);
        try {
            await loadTranscriptsAtOffset(offsetRef.current, true);
        } finally {
            setIsLoadingMore(false);
            isLoadingRef.current = false;
        }
    }, [hasMore, meetingId, loadTranscriptsAtOffset, isLoading]);

    // Force refetch of data (e.g., after retranscription)
    const refetch = useCallback(async () => {
        if (!meetingId) return;

        reset();
        setIsLoading(true);
        try {
            await loadMetadata();
            await loadTranscriptsAtOffset(0, false);
        } finally {
            setIsLoading(false);
        }
    }, [meetingId, reset, loadMetadata, loadTranscriptsAtOffset]);

    const updateSpeaker = useCallback(async (transcriptId: string, speaker: string | null) => {
        if (!meetingId) return;
        const nextSpeaker = normalizeSpeaker(speaker);
        setTranscripts(prev =>
            prev.map(t => t.id === transcriptId ? { ...t, speaker: nextSpeaker ?? undefined } : t)
        );
        try {
            await invoke('api_update_transcript_speaker', {
                meetingId,
                transcriptId,
                speaker: nextSpeaker,
            });
        } catch (err) {
            console.error('Failed to update transcript speaker:', err);
            setError('Failed to update speaker label');
            await refetch();
            throw err;
        }
    }, [meetingId, refetch]);

    const applySpeakerToMatching = useCallback(async (
        fromSpeaker: string | null | undefined,
        speaker: string | null,
    ): Promise<number> => {
        if (!meetingId) return 0;
        const currentSpeaker = normalizeSpeaker(fromSpeaker);
        const nextSpeaker = normalizeSpeaker(speaker);
        setTranscripts(prev =>
            prev.map(t => {
                const rowSpeaker = normalizeSpeaker(t.speaker);
                return rowSpeaker === currentSpeaker ? { ...t, speaker: nextSpeaker ?? undefined } : t;
            })
        );
        try {
            const response = await invoke<{ updated: number }>('api_update_transcript_speakers_matching', {
                meetingId,
                fromSpeaker: currentSpeaker,
                speaker: nextSpeaker,
            });
            return response.updated;
        } catch (err) {
            console.error('Failed to update matching transcript speakers:', err);
            setError('Failed to update matching speaker labels');
            await refetch();
            throw err;
        }
    }, [meetingId, refetch]);

    // Initial load
    useEffect(() => {
        if (!meetingId) {
            reset();
            return;
        }

        // Avoid reloading the same meeting
        if (loadedMeetingIdRef.current === meetingId) return;
        loadedMeetingIdRef.current = meetingId;

        reset();

        const loadInitial = async () => {
            setIsLoading(true);
            try {
                await loadMetadata();
                await loadTranscriptsAtOffset(0, false);
            } finally {
                setIsLoading(false);
            }
        };

        loadInitial();
    }, [meetingId, reset, loadMetadata, loadTranscriptsAtOffset]);

    // Convert to segments (memoized)
    const segments = useMemo(() =>
        convertTranscriptsToSegments(transcripts),
        [transcripts]
    );

    return {
        metadata,
        segments,
        transcripts,
        isLoading,
        isLoadingMore,
        hasMore,
        totalCount,
        loadedCount: transcripts.length,
        error,
        loadMore,
        reset,
        refetch,
        updateSpeaker,
        applySpeakerToMatching,
    };
}
