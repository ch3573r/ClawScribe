import { useState, useCallback, useRef, useEffect, useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Transcript, MeetingMetadata, PaginatedTranscriptsResponse, TranscriptSegmentData } from "@/types";
import type { TranscriptTextPatch } from "@/lib/meetingTranscripts";
import { toast } from "sonner";

const DEFAULT_PAGE_SIZE = 100;

type TimelineRow = Pick<Transcript, 'id' | 'text' | 'original_text' | 'audio_start_time' | 'audio_end_time' | 'speaker'>;

interface UsePaginatedTranscriptsProps {
    meetingId: string | null;
    /** Optional initial timestamp (in seconds) from URL for loading the correct page */
}

interface UsePaginatedTranscriptsReturn {
    metadata: MeetingMetadata | null;
    segments: TranscriptSegmentData[];
    timelineSegments: TranscriptSegmentData[];
    timelineComplete: boolean;
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
    patchTranscriptText: (patches: TranscriptTextPatch[]) => void;
    revealSource: (id: string, index: number) => Promise<void>;
    updateSpeaker: (transcriptId: string, speaker: string | null) => Promise<void>;
    applySpeakerToMatching: (fromSpeaker: string | null | undefined, speaker: string | null) => Promise<number>;
}

/**
 * Convert Transcript array to TranscriptSegmentData for virtualized display
 */
function convertTranscriptsToSegments(transcripts: (TimelineRow & Pick<Transcript, 'confidence' | 'word_timestamps'>)[]): TranscriptSegmentData[] {
    return transcripts.map(t => ({
        id: t.id,
        timestamp: t.audio_start_time ?? Number.NaN,
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

function mergeRows(current: Transcript[], incoming: Transcript[]) {
    const byId = new Map(current.map(row => [row.id, row]));
    incoming.forEach(row => byId.set(row.id, row));
    return [...byId.values()].sort((a, b) => {
        const left = a.audio_start_time ?? -Infinity;
        const right = b.audio_start_time ?? -Infinity;
        if (left !== right) return left - right;
        return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
    });
}

export function usePaginatedTranscripts({
    meetingId,
}: UsePaginatedTranscriptsProps): UsePaginatedTranscriptsReturn {
    const [metadata, setMetadata] = useState<MeetingMetadata | null>(null);
    const [transcripts, setTranscripts] = useState<Transcript[]>([]);
    const transcriptsRef = useRef(transcripts);
    transcriptsRef.current = transcripts;
    const [timelineSegments, setTimelineSegments] = useState<TranscriptSegmentData[]>([]);
    const [timelineComplete, setTimelineComplete] = useState(false);
    const [totalCount, setTotalCount] = useState(0);
    const [isLoading, setIsLoading] = useState(true);
    const [isLoadingMore, setIsLoadingMore] = useState(false);
    const [hasMore, setHasMore] = useState(false);
    const [error, setError] = useState<string | null>(null);

    const offsetRef = useRef(0);
    const generationRef = useRef(0);
    const isLoadingRef = useRef(false);
    const editVersion = useRef(0);
    const recentEdits = useRef(new Map<string, { version: number; patch: TranscriptTextPatch }>());
    const revealedRows = useRef<Transcript[]>([]);
    const pendingTimelineSpeakers = useRef<((row: TranscriptSegmentData) => TranscriptSegmentData)[] | null>(null);

    const patchRows = useCallback(<Row extends TimelineRow>(rows: Row[], after: number, lean = false): Row[] => rows.map(row => {
        const edit = recentEdits.current.get(row.id);
        if (!edit || edit.version <= after) return row;
        return {
            ...row,
            text: edit.patch.text,
            original_text: edit.patch.original_text ?? undefined,
            word_timestamps: lean ? undefined : edit.patch.word_timestamps ?? undefined,
        };
    }), []);

    const patchTranscriptText = useCallback((patches: TranscriptTextPatch[]) => {
        const after = editVersion.current;
        const version = ++editVersion.current;
        const byId = new Map(patches.map(patch => [patch.id, patch]));
        for (const patch of patches) recentEdits.current.set(patch.id, { version, patch });
        setTranscripts(rows => patchRows(rows, after));
        setTimelineSegments(rows => rows.map(row => {
            const patch = byId.get(row.id);
            return patch ? { ...row, text: patch.text, original_text: patch.original_text ?? undefined } : row;
        }));
    }, [patchRows]);

    // Reset state when meeting changes
    const reset = useCallback(() => {
        generationRef.current++;
        recentEdits.current.clear();
        revealedRows.current = [];
        pendingTimelineSpeakers.current = null;
        isLoadingRef.current = false;
        setMetadata(null);
        setTranscripts([]);
        setTimelineSegments([]);
        setTimelineComplete(false);
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
            const version = editVersion.current;
            const response = await invoke<PaginatedTranscriptsResponse>(
                'api_get_meeting_transcripts',
                {
                    meetingId,
                    limit: DEFAULT_PAGE_SIZE,
                    offset,
                }
            );

            if (generation !== generationRef.current) return [];
            const newTranscripts = patchRows(response.transcripts, version);

            if (append) {
                setTranscripts(prev => mergeRows(prev, newTranscripts));
            } else {
                const sourced = patchRows(revealedRows.current, 0);
                setTranscripts(sourced.length ? mergeRows(sourced, newTranscripts) : newTranscripts);
                revealedRows.current = [];
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
    }, [meetingId, patchRows]);

    const loadTimeline = useCallback(async (generation: number) => {
        if (!meetingId) return;
        const speakerUpdates: ((row: TranscriptSegmentData) => TranscriptSegmentData)[] = [];
        pendingTimelineSpeakers.current = speakerUpdates;
        try {
            const version = editVersion.current;
            const rows = await invoke<TimelineRow[]>('api_get_meeting_transcript_timeline', { meetingId });
            if (generation !== generationRef.current) return;
            const timeline = convertTranscriptsToSegments(patchRows(rows, version, true));
            setTimelineSegments(speakerUpdates.length ? timeline.map(row => speakerUpdates.reduce((current, update) => update(current), row)) : timeline);
            setTimelineComplete(true);
        } catch {
            if (generation !== generationRef.current) return;
            setTimelineSegments([]);
            toast.error('Failed to load speaker timeline. Reopen the meeting to retry.');
        } finally {
            if (pendingTimelineSpeakers.current === speakerUpdates) pendingTimelineSpeakers.current = null;
        }
    }, [meetingId, patchRows]);

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
        recentEdits.current.clear();
        revealedRows.current = [];
        offsetRef.current = 0;
        isLoadingRef.current = true;
        setIsLoading(true);
        setIsLoadingMore(false);
        setError(null);
        setTimelineComplete(false);
        setTimelineSegments([]);
        const firstPage = Promise.all([loadMetadata(generation), loadTranscriptsAtOffset(0, false, generation)]);
        void loadTimeline(generation);
        try {
            await firstPage;
        } finally {
            if (generation === generationRef.current) {
                setIsLoading(false);
                isLoadingRef.current = false;
            }
        }
    }, [meetingId, loadMetadata, loadTranscriptsAtOffset, loadTimeline]);

    const updateSpeaker = useCallback(async (transcriptId: string, speaker: string | null) => {
        if (!meetingId) return;
        const generation = generationRef.current;
        const nextSpeaker = normalizeSpeaker(speaker);
        const update = (row: TranscriptSegmentData) => row.id === transcriptId ? { ...row, speaker: nextSpeaker ?? undefined } : row;
        pendingTimelineSpeakers.current?.push(update);
        setTranscripts(prev =>
            prev.map(t => t.id === transcriptId ? { ...t, speaker: nextSpeaker ?? undefined } : t)
        );
        setTimelineSegments(prev => prev.map(update));
        try {
            await invoke('api_update_transcript_speaker', {
                meetingId,
                transcriptId,
                speaker: nextSpeaker,
            });
        } catch (err) {
            if (generation !== generationRef.current) throw err;
            console.error('Failed to update transcript speaker:', err);
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
        const update = (row: TranscriptSegmentData) => normalizeSpeaker(row.speaker) === currentSpeaker ? { ...row, speaker: nextSpeaker ?? undefined } : row;
        pendingTimelineSpeakers.current?.push(update);
        setTranscripts(prev =>
            prev.map(t => {
                const rowSpeaker = normalizeSpeaker(t.speaker);
                return rowSpeaker === currentSpeaker ? { ...t, speaker: nextSpeaker ?? undefined } : t;
            })
        );
        setTimelineSegments(prev => prev.map(update));
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
            await refetch();
            throw err;
        }
    }, [meetingId, refetch]);

    useEffect(() => {
        reset();
        if (meetingId) void refetch();
        else setIsLoading(false);
        return () => { generationRef.current += 1; };
    }, [meetingId, reset, refetch]);

    // Fetch the cited page without advancing sequential pagination past unloaded pages.
    const revealSource = useCallback(async (id: string, index: number) => {
        if (!meetingId || !Number.isInteger(index) || index < 0) throw new Error('Invalid source position.');
        if (transcriptsRef.current.some(row => row.id === id)) return;
        const generation = generationRef.current;
        const version = editVersion.current;
        const response = await invoke<PaginatedTranscriptsResponse>('api_get_meeting_transcripts', {
            meetingId, limit: DEFAULT_PAGE_SIZE, offset: Math.floor(index / DEFAULT_PAGE_SIZE) * DEFAULT_PAGE_SIZE,
        });
        if (generation !== generationRef.current) return;
        if (!response.transcripts.some(row => row.id === id)) throw new Error('The transcript changed. Open the source reference again.');
        const rows = patchRows(response.transcripts, version);
        if (!offsetRef.current && isLoadingRef.current) revealedRows.current = mergeRows(revealedRows.current, rows);
        setTranscripts(previous => mergeRows(previous, rows));
    }, [meetingId, patchRows]);

    // Convert to segments (memoized)
    const segments = useMemo(() =>
        convertTranscriptsToSegments(transcripts),
        [transcripts]
    );

    return {
        metadata,
        segments,
        timelineSegments,
        timelineComplete,
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
        patchTranscriptText,
        revealSource,
        updateSpeaker,
        applySpeakerToMatching,
    };
}
