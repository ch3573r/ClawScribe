import { useCallback, useEffect, useRef, useState } from "react";

/**
 * Keeps text the user is typing local, and commits it once they pause, press
 * Enter or leave the field. Whatever applies the text (a stored setting, a font
 * on the whole page) then changes once per edit instead of once per keystroke.
 * A pending edit is committed when the component unmounts, so it is never lost.
 */
export function useDraftCommit(value: string, onCommit: (text: string) => void, delayMs = 500) {
  const [draft, setDraft] = useState(value);
  const latest = useRef({ value, draft, onCommit });
  latest.current = { value, draft, onCommit };

  // The committed value changed elsewhere (or this edit was committed).
  useEffect(() => { setDraft(value); }, [value]);

  const commit = useCallback(() => {
    const { value: committed, draft: text, onCommit: apply } = latest.current;
    if (text !== committed) apply(text);
  }, []);

  useEffect(() => {
    if (draft === value) return;
    const timer = setTimeout(commit, delayMs);
    return () => clearTimeout(timer);
  }, [draft, value, delayMs, commit]);

  useEffect(() => () => commit(), [commit]);

  return { draft, setDraft, commit, pending: draft !== value };
}
