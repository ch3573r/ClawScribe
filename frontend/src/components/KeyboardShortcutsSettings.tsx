"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AlertTriangle, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Kbd, SettingsCard, SettingsFooter, SettingsRow, SettingsStack } from "@/components/ui/settings-layout";
import { NoteRow } from "@/components/settings/settings-parts";
import {
  captureAccelerator,
  DEFAULT_BINDINGS,
  duplicateAction,
  isDefaultBindings,
  keycaps,
  RESERVED_SHORTCUTS,
  SHORTCUT_ACTIONS,
  type ShortcutAction,
  type ShortcutBindings,
} from "@/lib/shortcuts";

interface ShortcutApplyResult {
  bindings: ShortcutBindings;
  conflicts: string[];
}

const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));

export function KeyboardShortcutsSettings() {
  const [bindings, setBindings] = useState<ShortcutBindings | null>(null);
  const [capturing, setCapturing] = useState<ShortcutAction | null>(null);
  const [conflicts, setConflicts] = useState<string[]>([]);
  const [applying, setApplying] = useState<ShortcutAction | "all" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState("");
  const [readFailed, setReadFailed] = useState(false);
  const bindingsRef = useRef<ShortcutBindings | null>(null);
  bindingsRef.current = bindings;

  const load = useCallback(() => {
    invoke<ShortcutBindings>("get_shortcuts")
      .then((saved) => { setReadFailed(false); setBindings(saved); })
      .catch(() => {
        setReadFailed(true);
        setBindings({ ...DEFAULT_BINDINGS });
      });
  }, []);

  useEffect(() => { load(); }, [load]);

  /** Register `next` at once. If it fails, the previous shortcuts stay and the error is shown. */
  const apply = useCallback(async (next: ShortcutBindings, changed: ShortcutAction | "all") => {
    const previous = bindingsRef.current;
    setApplying(changed);
    setError(null);
    setNotice("");
    try {
      const result = await invoke<ShortcutApplyResult>("set_shortcuts", { bindings: next });
      if (changed !== "all" && result.conflicts.includes(changed) && previous) {
        // The new combination could not be registered. Put the previous set back.
        try {
          const restored = await invoke<ShortcutApplyResult>("set_shortcuts", { bindings: previous });
          setConflicts(restored.conflicts);
        } catch { /* the error below already says the previous shortcut is kept */ }
        const label = SHORTCUT_ACTIONS.find((action) => action.key === changed)?.label ?? "This shortcut";
        setError(`${label}: that combination is in use by Windows or another app. The previous shortcut is kept.`);
        return;
      }
      setBindings(result.bindings);
      setConflicts(result.conflicts);
      setNotice("Shortcut updated");
    } catch (e) {
      setError(`Could not apply the shortcut. The previous one is kept. ${errorText(e)}`.trim());
    } finally {
      setApplying(null);
    }
  }, []);

  useEffect(() => {
    if (!capturing) return;
    const handler = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape") {
        setCapturing(null);
        return;
      }
      const captured = captureAccelerator(e);
      if (captured.kind === "ignore") return;
      if (captured.kind === "needs-modifier") {
        setError("Press Ctrl, Alt, Shift or Win together with another key.");
        return;
      }
      const current = bindingsRef.current;
      if (!current) return;
      const action = capturing;
      setCapturing(null);
      if (current[action] === captured.accelerator) { setError(null); return; }
      const duplicate = duplicateAction(current, action, captured.accelerator);
      if (duplicate) {
        const other = SHORTCUT_ACTIONS.find((item) => item.key === duplicate)?.label ?? "another action";
        setError(`That combination is already used for "${other}". Pick a different one.`);
        return;
      }
      void apply({ ...current, [action]: captured.accelerator }, action);
    };
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [capturing, apply]);

  if (!bindings) {
    return (
      <SettingsStack>
        <SettingsCard>
          <SettingsFooter><span role="status" className="inline-flex items-center gap-2"><Loader2 aria-hidden="true" className="h-3.5 w-3.5 animate-spin" />Loading shortcuts…</span></SettingsFooter>
        </SettingsCard>
      </SettingsStack>
    );
  }

  const busy = applying !== null;
  return (
    <SettingsStack>
      <SettingsCard>
        {SHORTCUT_ACTIONS.map(({ key, label }) => {
          const accelerator = bindings[key];
          const reserved = RESERVED_SHORTCUTS.has(accelerator);
          const conflicted = conflicts.includes(key);
          const isCapturing = capturing === key;
          return (
            <SettingsRow
              key={key}
              title={label}
              description={(reserved || conflicted) && (
                <span className="inline-flex items-center gap-1.5 text-warning-foreground">
                  <AlertTriangle aria-hidden="true" className="h-3.5 w-3.5 shrink-0" />
                  {conflicted ? "In use by Windows or another app, so it is not active" : "This is a reserved Windows shortcut"}
                </span>
              )}
            >
              <button
                type="button"
                disabled={busy || readFailed}
                aria-label={isCapturing ? `${label}: press the new keys, or Escape to cancel` : `${label}: ${keycaps(accelerator).join(" ")}. Click to change`}
                onClick={() => { setError(null); setNotice(""); setCapturing(isCapturing ? null : key); }}
                onBlur={() => setCapturing((current) => (current === key ? null : current))}
                className="flex min-h-[34px] items-center gap-1.5 rounded-md px-1.5 py-1 transition-colors hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-60"
              >
                {isCapturing ? (
                  <span className="px-1.5 text-[13px] text-primary">Press the new keys · Esc to cancel</span>
                ) : applying === key ? (
                  <Loader2 aria-hidden="true" className="mx-3 h-4 w-4 animate-spin text-muted-foreground" />
                ) : (
                  keycaps(accelerator).map((cap, index) => <Kbd key={`${cap}-${index}`}>{cap}</Kbd>)
                )}
              </button>
            </SettingsRow>
          );
        })}
        {readFailed && (
          <NoteRow tone="warning">
            The saved shortcuts could not be read, so they cannot be changed one at a time. Your shortcuts still work.{" "}
            <Button type="button" variant="link" size="sm" className="h-auto p-0 align-baseline" onClick={load}>Try again</Button>
          </NoteRow>
        )}
        {error && <NoteRow tone="error">{error}</NoteRow>}
        <SettingsFooter
          actions={
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={busy || capturing !== null || (!readFailed && isDefaultBindings(bindings))}
              onClick={() => { setCapturing(null); void apply({ ...DEFAULT_BINDINGS }, "all"); }}
            >
              {applying === "all" ? <Loader2 aria-hidden="true" className="animate-spin" /> : null}
              Reset to defaults
            </Button>
          }
        >
          Click a shortcut and press the new keys. Changes apply immediately
        </SettingsFooter>
        <span role="status" className="sr-only">{notice}</span>
      </SettingsCard>
    </SettingsStack>
  );
}
