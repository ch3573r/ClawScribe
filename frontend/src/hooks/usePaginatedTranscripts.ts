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
    revealSource: (id: string, index: number) => Promise<void>;
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
        original_text: t.original_text,
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
    const generationRef = useRef(0);
    const isLoadingRef = useRef(false);

    // Reset state when meeting changes
    const reset = useCallback(() => {
        generationRef.current++;
        isLoadingRef.current = false;
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
    const loadMetadata = useCallback(async (generation: number): Promise<MeetingMetadata | null> => {
        if (!meetingId) return null;

        try {
            const data = await invoke<MeetingMetadata>('api_get_meeting_metadata', {
                meetingId,
            });
            if (generation !== generationRef.current) return null;
            setMetadata(data);
            return data;
        } catch (err) {
            if (generation !== generationRef.current) return null;
            console.error('Failed to load meeting metadata:', err);
            setError('Failed to load meeting details');
            return null;
        }
    }, [meetingId]);

    // Load transcripts at specific offset
    const loadTranscriptsAtOffset = useCallback(async (
        offset: number,
        append: boolean,
        generation: number,
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

            if (generation !== generationRef.current) return [];
            const newTranscripts = response.transcripts;

            if (append) {
                setTranscripts(prev => {
                    // Deduplicate by id
                    const existingIds = new Set(prev.map(t => t.id));
                    const uniqueNew = newTranscripts.filter(t => !existingIds.has(t.id));
                    // Sort by audio_start_time
                    return [...prev, ...uniqueNew].sort((a, b) =>
                        (a.audio_start_time ?? 0) - (b.audio_start_time ?? 0) || a.id.localeCompare(b.id)
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
            if (generation !== generationRef.current) return [];
            console.error('Failed to load transcripts:', err);
            setError('Failed to load transcripts');
            return [];
        }
    }, [meetingId]);

    // A synchronous lock prevents duplicate pages before React renders the loading state.
    const loadMore = useCallback(async () => {
        if (isLoadingRef.current || !hasMore || !meetingId || isLoading) return;
        const generation = generationRef.current;
        isLoadingRef.current = true;
        setIsLoadingMore(true);
        try {
            await loadTranscriptsAtOffset(offsetRef.current, true, generation);
        } finally {
            if (generation === generationRef.current) {
                setIsLoadingMore(false);
                isLoadingRef.current = false;
            }
        }
    }, [hasMore, meetingId, loadTranscriptsAtOffset, isLoading]);

    // Invalidates every earlier page, including source pages and previous refetches.
    const refetch = useCallback(async () => {
        const generation = ++generationRef.current;
        if (!meetingId) return;
        offsetRef.current = 0;
        isLoadingRef.current = true;
        setIsLoading(true);
        setIsLoadingMore(false);
        setError(null);
        try {
            await Promise.all([
                loadMetadata(generation),
                loadTranscriptsAtOffset(0, false, generation),
            ]);
        } finally {
            if (generation === generationRef.current) {
                setIsLoading(false);
                isLoadingRef.current = false;
            }
        }
    }, [meetingId, loadMetadata, loadTranscriptsAtOffset]);

    const updateSpeaker = useCallback(async (transcriptId: string, speaker: string | null) => {
        if (!meetingId) return;
        const generation = generationRef.current;
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
            if (generation !== generationRef.current) throw err;
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
        const generation = generationRef.current;
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
            if (generation !== generationRef.current) throw err;
            console.error('Failed to update matching transcript speakers:', err);
            setError('Failed to update matching speaker labels');
            await refetch();
            throw err;
        }
    }, [meetingId, refetch]);

    useEffect(() => {
        reset();
        if (meetingId) void refetch();
        else setIsLoading(false);
        return () => { generationRef.current++; };
    }, [meetingId, reset, refetch]);

    // Fetch the cited page without advancing sequential pagination past unloaded pages.
    const revealSource = useCallback(async (id: string, index: number) => {
        if (!meetingId || !Number.isInteger(index) || index < 0) throw new Error('Invalid source position.');
        if (transcripts.some(row => row.id === id)) return;
        const generation = generationRef.current;
        const response = await invoke<PaginatedTranscriptsResponse>('api_get_meeting_transcripts', {
            meetingId, limit: DEFAULT_PAGE_SIZE, offset: Math.floor(index / DEFAULT_PAGE_SIZE) * DEFAULT_PAGE_SIZE,
        });
        if (generation !== generationRef.current) return;
        if (!response.transcripts.some(row => row.id === id)) throw new Error('The transcript changed. Open the source reference again.');
        setTranscripts(previous => {
            const byId = new Map(previous.map(row => [row.id, row]));
            response.transcripts.forEach(row => byId.set(row.id, row));
            return [...byId.values()].sort((a, b) => (a.audio_start_time ?? 0) - (b.audio_start_time ?? 0) || a.id.localeCompare(b.id));
        });
    }, [meetingId, transcripts]);

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
        revealSource,
        updateSpeaker,
        applySpeakerToMatching,
    };
}
