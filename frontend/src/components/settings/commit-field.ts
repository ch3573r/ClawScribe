import { useEffect, useRef, useState } from "react";

/**
 * Immediate-save helpers for the Summary settings page.
 *
 * Selects and switches save as soon as they change (`useSaveFeedback`). Text
 * fields keep a draft and save on blur or Enter, only when valid
 * (`useCommitField`). A failed save keeps what the user typed and shows an
 * error. Only messages written as `SaveError` are ever displayed, so a raw
 * backend string, and any key inside it, never reaches the screen.
 */

export class SaveError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "SaveError";
  }
}

export const GENERIC_SAVE_ERROR = "Could not save. Try again";

/**
 * A short backend string that is safe to show, or the fallback. Anything long
 * or containing one of the given secrets is replaced by the fallback.
 */
export function safeMessage(error: unknown, fallback: string, secrets: Array<string | null | undefined> = []): string {
  const raw = typeof error === "string" ? error : error instanceof Error ? error.message : "";
  const text = raw.trim();
  if (!text || text.length > 200) return fallback;
  if (secrets.some(secret => !!secret && secret.trim().length >= 4 && text.includes(secret.trim()))) return fallback;
  return text;
}

export const messageOf = (error: unknown) => (error instanceof SaveError ? error.message : GENERIC_SAVE_ERROR);

/** Resolve with "held" when the value was kept but not saved yet (a required sibling is still empty). */
export type CommitResult = void | "held";

const SAVED_MS = 2000;

function unref(timer: unknown) {
  if (timer && typeof timer === "object" && "unref" in timer) (timer as { unref: () => void }).unref();
}

export interface CommitFieldOptions {
  /** The saved value. The draft follows it whenever the user is not editing. */
  value: string;
  /** Return a short message when the value cannot be saved. Runs before the save. */
  validate?: (value: string) => string | null;
  /** Save the value. Throw a `SaveError` to show a message; anything else shows a generic one. */
  onCommit: (value: string) => Promise<CommitResult>;
  normalize?: (value: string) => string;
  /** Write-only fields such as tokens are emptied after a save. */
  clearAfterSave?: boolean;
}

export function useCommitField({ value, validate, onCommit, normalize = text => text.trim(), clearAfterSave = false }: CommitFieldOptions) {
  const [draft, setDraft] = useState(value);
  const [error, setError] = useState("");
  const [status, setStatus] = useState<"idle" | "saving" | "saved">("idle");
  const draftRef = useRef(value);
  const dirty = useRef(false);
  const inFlight = useRef(false);
  const committed = useRef(normalize(value));
  const timer = useRef<unknown>(null);
  const latest = useRef({ validate, onCommit, normalize, clearAfterSave });
  latest.current = { validate, onCommit, normalize, clearAfterSave };

  useEffect(() => {
    committed.current = normalize(value);
    if (!dirty.current) { draftRef.current = value; setDraft(value); }
  }, [value]);
  useEffect(() => () => { if (timer.current) clearTimeout(timer.current as never); }, []);

  const change = (next: string) => {
    draftRef.current = next;
    dirty.current = true;
    setDraft(next);
    setError("");
  };

  const commit = async () => {
    if (inFlight.current) return;
    const current = latest.current;
    const started = draftRef.current;
    const next = current.normalize(started);
    if (next === committed.current) {
      dirty.current = false;
      if (started !== next) { draftRef.current = next; setDraft(next); }
      setError("");
      return;
    }
    const problem = current.validate?.(next);
    if (problem) { setError(problem); return; }
    inFlight.current = true;
    setStatus("saving");
    setError("");
    try {
      const result = await current.onCommit(next);
      // A write-only value (a key) is not kept in memory once it is saved.
      committed.current = current.clearAfterSave && result !== "held" ? "" : next;
      // Do not overwrite what the user typed while the save was running.
      if (draftRef.current === started) {
        dirty.current = false;
        const shown = current.clearAfterSave && result !== "held" ? "" : next;
        draftRef.current = shown;
        setDraft(shown);
      }
      if (result === "held") {
        setStatus("idle");
      } else {
        setStatus("saved");
        if (timer.current) clearTimeout(timer.current as never);
        timer.current = setTimeout(() => setStatus(state => (state === "saved" ? "idle" : state)), SAVED_MS);
        unref(timer.current);
      }
    } catch (failure) {
      setError(messageOf(failure));
      setStatus("idle");
    } finally {
      inFlight.current = false;
    }
  };

  const inputProps = {
    value: draft,
    onChange: (event: { target: { value: string } }) => change(event.target.value),
    onBlur: () => { void commit(); },
    onKeyDown: (event: { key: string; preventDefault: () => void; nativeEvent?: { isComposing?: boolean } }) => {
      if (event.key === "Enter" && !event.nativeEvent?.isComposing) { event.preventDefault(); void commit(); }
    },
    readOnly: status === "saving",
    "aria-invalid": error ? true : undefined,
  };

  return { draft, status, error, saving: status === "saving", change, commit, inputProps };
}

export type FeedbackState = { status: "saving" | "saved" | "error"; message?: string };

/** Per-control feedback for selects and switches that save the moment they change. */
export function useSaveFeedback() {
  const [states, setStates] = useState<Record<string, FeedbackState>>({});
  const timers = useRef<Record<string, unknown>>({});
  useEffect(() => () => { Object.values(timers.current).forEach(timer => clearTimeout(timer as never)); }, []);

  const run = async (key: string, work: () => Promise<CommitResult>): Promise<boolean> => {
    setStates(previous => ({ ...previous, [key]: { status: "saving" } }));
    try {
      // A held value was kept but not saved, so it gets no "saved" confirmation.
      if ((await work()) === "held") {
        setStates(previous => ({ ...previous, [key]: undefined as never }));
        return true;
      }
      setStates(previous => ({ ...previous, [key]: { status: "saved" } }));
      if (timers.current[key]) clearTimeout(timers.current[key] as never);
      const timer = setTimeout(() => setStates(previous => (previous[key]?.status === "saved" ? { ...previous, [key]: undefined as never } : previous)), SAVED_MS);
      unref(timer);
      timers.current[key] = timer;
      return true;
    } catch (failure) {
      setStates(previous => ({ ...previous, [key]: { status: "error", message: messageOf(failure) } }));
      return false;
    }
  };

  const busy = Object.values(states).some(state => state?.status === "saving");
  return { states, busy, run };
}
