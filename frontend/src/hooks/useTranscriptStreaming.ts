import { useState, useEffect } from 'react';
import { TranscriptSegmentData } from '@/types';

const INTERVAL_MS = 15; // Character reveal interval
const DURATION_MS = 800; // Total streaming duration
const INITIAL_CHARS = 5; // Show first N characters immediately

interface StreamingSegment {
  id: string;
  fullText: string;
  visibleText: string;
}

/**
 * Hook to manage the typewriter/streaming effect for new transcripts
 * Gradually reveals characters in a transcript over 800ms
 */
export function useTranscriptStreaming(
  segments: TranscriptSegmentData[],
  isRecording: boolean,
  enableStreaming: boolean
) {
  const [streamingSegment, setStreamingSegment] = useState<StreamingSegment | null>(null);
  const latestSegment = segments[segments.length - 1];
  const latestId = latestSegment?.id;
  const latestText = latestSegment?.text;

  useEffect(() => {
    if (!isRecording || !enableStreaming || latestId === undefined || latestText === undefined) {
      setStreamingSegment(null);
      return;
    }

    const fullText = latestText;
    let charIndex = Math.min(INITIAL_CHARS, fullText.length);
    setStreamingSegment({ id: latestId, fullText, visibleText: fullText.substring(0, charIndex) });
    if (charIndex === fullText.length) return;

    const totalTicks = Math.floor(DURATION_MS / INTERVAL_MS);
    const charsPerTick = Math.max(2, Math.ceil((fullText.length - INITIAL_CHARS) / totalTicks));
    const timer = setInterval(() => {
      charIndex = Math.min(fullText.length, charIndex + charsPerTick);
      setStreamingSegment({ id: latestId, fullText, visibleText: fullText.substring(0, charIndex) });
      if (charIndex === fullText.length) clearInterval(timer);
    }, INTERVAL_MS);
    return () => clearInterval(timer);
  }, [latestId, latestText, isRecording, enableStreaming]);

  /**
   * Get the display text for a segment, with streaming effect if applicable
   */
  const getDisplayText = (segment: TranscriptSegmentData): string => {
    if (isRecording && enableStreaming && streamingSegment && segment.id === streamingSegment.id && segment.text === streamingSegment.fullText) {
      return streamingSegment.visibleText;
    }
    return segment.text;
  };

  return {
    streamingSegmentId: streamingSegment?.id ?? null,
    getDisplayText,
  };
}
