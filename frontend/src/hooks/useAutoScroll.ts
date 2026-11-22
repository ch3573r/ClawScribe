import { useCallback, useLayoutEffect, useRef, useState, type RefObject } from "react";

interface UseAutoScrollProps {
    scrollRef: RefObject<HTMLDivElement | null>;
    contentRef: RefObject<HTMLDivElement | null>;
    segmentCount: number;
    enabled: boolean;
    isPaused: boolean;
}

// Scroll offsets can be fractional even though scrollHeight and clientHeight are integers.
const SCROLL_TOLERANCE = 1;

/** Keep the live tail in view until the reader scrolls back. */
export function useAutoScroll({ scrollRef, contentRef, segmentCount, enabled, isPaused }: UseAutoScrollProps) {
    const [autoScroll, setAutoScroll] = useState(true);
    const following = useRef(true);
    const previousTop = useRef(0);
    const hasSegments = segmentCount > 0;

    const pinToTail = useCallback(() => {
        const container = scrollRef.current;
        if (!container) return;
        container.scrollTop = container.scrollHeight;
        previousTop.current = container.scrollTop;
    }, [scrollRef]);

    const scrollToBottom = useCallback(() => {
        following.current = true;
        setAutoScroll(true);
        pinToTail();
        scrollRef.current?.focus({ preventScroll: true });
    }, [pinToTail, scrollRef]);

    useLayoutEffect(() => {
        if (!enabled) {
            following.current = true;
            setAutoScroll(true);
            return;
        }
        const container = scrollRef.current;
        if (!container) return;
        previousTop.current = container.scrollTop;

        const detach = () => {
            if (!following.current) return;
            following.current = false;
            setAutoScroll(false);
        };
        const handleScroll = () => {
            const { scrollTop, scrollHeight, clientHeight } = container;
            if (scrollTop < previousTop.current - SCROLL_TOLERANCE &&
                scrollHeight - clientHeight - scrollTop > SCROLL_TOLERANCE) {
                detach();
            }
            previousTop.current = scrollTop;
        };
        // Wheel and keyboard intent can arrive before the browser delivers its
        // scroll event. A transcript commit in that gap must yield to the reader.
        const handleWheel = (event: WheelEvent) => {
            if (!event.ctrlKey && event.deltaY < 0 && container.scrollTop > 0) detach();
        };
        const handleKeyDown = (event: KeyboardEvent) => {
            if (event.defaultPrevented || event.target !== container || container.scrollTop <= 0) return;
            if (["ArrowUp", "PageUp", "Home"].includes(event.key) || (event.key === " " && event.shiftKey)) detach();
        };

        container.addEventListener("scroll", handleScroll, { passive: true });
        container.addEventListener("wheel", handleWheel, { passive: true });
        container.addEventListener("keydown", handleKeyDown);
        return () => {
            container.removeEventListener("scroll", handleScroll);
            container.removeEventListener("wheel", handleWheel);
            container.removeEventListener("keydown", handleKeyDown);
        };
    }, [enabled, scrollRef]);

    useLayoutEffect(() => {
        if (enabled && !isPaused && segmentCount > 0 && following.current) pinToTail();
    }, [enabled, isPaused, segmentCount, pinToTail]);

    useLayoutEffect(() => {
        const container = scrollRef.current;
        const content = contentRef.current;
        if (!enabled || isPaused || !container || !content || !hasSegments) return;

        // Wrapped streaming text and virtual row measurements can change the
        // tail after the transcript commit, including when the window is resized.
        const observer = new window.ResizeObserver(() => {
            if (following.current) pinToTail();
        });
        observer.observe(container);
        observer.observe(content);
        return () => observer.disconnect();
    }, [enabled, isPaused, hasSegments, scrollRef, contentRef, pinToTail]);

    return { autoScroll, scrollToBottom };
}
