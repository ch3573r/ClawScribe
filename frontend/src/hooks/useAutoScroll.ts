import { useRef, useState, useEffect, useCallback, type RefObject } from "react";
import type { Virtualizer } from "@tanstack/react-virtual";

interface UseAutoScrollProps {
    scrollRef: RefObject<HTMLDivElement | null>;
    segments: readonly { id: string }[];
    isRecording: boolean;
    isPaused: boolean;
    activeSegmentId?: string;
    virtualizer?: Virtualizer<HTMLDivElement, Element>;
    virtualizationThreshold?: number;
    disableAutoScroll?: boolean;
}

interface UseAutoScrollReturn {
    autoScroll: boolean;
    setAutoScroll: (value: boolean) => void;
    scrollToBottom: () => void;
}

const SCROLL_THRESHOLD = 100;

/** Follow live speech without pulling the user away from earlier transcript text. */
export function useAutoScroll({
    scrollRef,
    segments,
    isRecording,
    isPaused,
    activeSegmentId,
    virtualizer,
    virtualizationThreshold = 10,
    disableAutoScroll = false,
}: UseAutoScrollProps): UseAutoScrollReturn {
    const useVirtualization = Boolean(virtualizer && segments.length >= virtualizationThreshold);
    const [autoScroll, setAutoScrollState] = useState(true);
    const autoScrollRef = useRef(true);
    const isProgrammaticScrollRef = useRef(false);
    const prevSegmentCountRef = useRef(segments.length);
    // These are browser timers; Node's ambient overload can mislead ReturnType.
    const resetTimerRef = useRef<number | null>(null);
    const settleTimerRef = useRef<number | null>(null);

    const setAutoScroll = useCallback((value: boolean) => {
        // Update immediately: another transcript event can arrive before React renders.
        autoScrollRef.current = value;
        setAutoScrollState(value);
    }, []);

    const clearPendingScroll = useCallback(() => {
        if (resetTimerRef.current !== null) window.clearTimeout(resetTimerRef.current);
        if (settleTimerRef.current !== null) window.clearTimeout(settleTimerRef.current);
        resetTimerRef.current = null;
        settleTimerRef.current = null;
        isProgrammaticScrollRef.current = false;
    }, []);

    const finishProgrammaticScroll = useCallback((delay: number) => {
        if (resetTimerRef.current !== null) window.clearTimeout(resetTimerRef.current);
        resetTimerRef.current = window.setTimeout(() => {
            resetTimerRef.current = null;
            isProgrammaticScrollRef.current = false;
        }, delay);
    }, []);

    const scrollToBottom = useCallback(() => {
        const container = scrollRef.current;
        if (!container) return;
        clearPendingScroll();
        isProgrammaticScrollRef.current = true;
        container.scrollTop = container.scrollHeight;
        setAutoScroll(true);
        finishProgrammaticScroll(50);
    }, [scrollRef, clearPendingScroll, setAutoScroll, finishProgrammaticScroll]);

    useEffect(() => clearPendingScroll, [clearPendingScroll]);

    useEffect(() => {
        const container = scrollRef.current;
        if (!container) return;

        const handleScroll = () => {
            if (isProgrammaticScrollRef.current) return;
            const nearBottom = container.scrollHeight - container.scrollTop - container.clientHeight <= SCROLL_THRESHOLD;
            if (nearBottom !== autoScrollRef.current) setAutoScroll(nearBottom);
        };
        // A user gesture must interrupt even the virtualizer's deferred follow-up.
        const handleUserInput = () => clearPendingScroll();
        const handleKeyDown = (event: KeyboardEvent) => {
            if (["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "].includes(event.key)) {
                handleUserInput();
            }
        };

        container.addEventListener("scroll", handleScroll, { passive: true });
        container.addEventListener("wheel", handleUserInput, { passive: true });
        container.addEventListener("touchstart", handleUserInput, { passive: true });
        container.addEventListener("pointerdown", handleUserInput, { passive: true });
        container.addEventListener("keydown", handleKeyDown);
        return () => {
            container.removeEventListener("scroll", handleScroll);
            container.removeEventListener("wheel", handleUserInput);
            container.removeEventListener("touchstart", handleUserInput);
            container.removeEventListener("pointerdown", handleUserInput);
            container.removeEventListener("keydown", handleKeyDown);
            clearPendingScroll();
        };
    }, [scrollRef, setAutoScroll, clearPendingScroll]);

    useEffect(() => {
        const hasNewSegments = segments.length > prevSegmentCountRef.current;
        prevSegmentCountRef.current = segments.length;
        clearPendingScroll();
        if (disableAutoScroll || !hasNewSegments || !autoScrollRef.current || !isRecording || isPaused) return;
        const container = scrollRef.current;
        if (!container) return;

        // Use the position remembered BEFORE this render. Rechecking scrollHeight
        // after a tall row is appended falsely treats it as the user scrolling up.
        isProgrammaticScrollRef.current = true;
        if (useVirtualization && virtualizer) {
            virtualizer.scrollToOffset(virtualizer.getTotalSize(), { align: "end" });
            settleTimerRef.current = window.setTimeout(() => {
                settleTimerRef.current = null;
                if (autoScrollRef.current && scrollRef.current === container) {
                    container.scrollTop = container.scrollHeight;
                }
            }, 50);
        } else {
            container.scrollTop = container.scrollHeight;
        }
        finishProgrammaticScroll(150);
        return clearPendingScroll;
    }, [segments.length, isRecording, isPaused, useVirtualization, virtualizer, scrollRef, disableAutoScroll, clearPendingScroll, finishProgrammaticScroll]);

    // Explicit search-result navigation remains available when live following is disabled.
    // Depending on the index rather than the array avoids re-scrolling on every append.
    const activeIndex = activeSegmentId ? segments.findIndex(segment => segment.id === activeSegmentId) : -1;
    useEffect(() => {
        if (!activeSegmentId || activeIndex < 0) return;
        clearPendingScroll();
        isProgrammaticScrollRef.current = true;
        if (useVirtualization && virtualizer) {
            // Smooth scrolling is unreliable while variable-height rows are measured.
            virtualizer.scrollToIndex(activeIndex, { align: "center", behavior: "auto" });
        } else {
            const reducedMotion = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
            const element = scrollRef.current?.ownerDocument.getElementById(`segment-${activeSegmentId}`);
            element?.scrollIntoView({ behavior: reducedMotion ? "auto" : "smooth", block: "center" });
        }
        finishProgrammaticScroll(500);
        return clearPendingScroll;
    }, [activeSegmentId, activeIndex, useVirtualization, virtualizer, scrollRef, clearPendingScroll, finishProgrammaticScroll]);

    return { autoScroll, setAutoScroll, scrollToBottom };
}
