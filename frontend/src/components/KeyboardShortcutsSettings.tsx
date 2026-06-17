"use client";

import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { AlertTriangle, Keyboard } from "lucide-react";
import { Button } from "@/components/ui/button";

interface ShortcutBindings {
  startStop: string;
  pauseResume: string;
}
interface ShortcutApplyResult {
  bindings: ShortcutBindings;
  conflicts: string[];
}

type ActionKey = keyof ShortcutBindings;

const ACTIONS: { key: ActionKey; label: string; description: string }[] = [
  { key: "startStop", label: "Start / stop recording", description: "Toggles recording from anywhere." },
  { key: "pauseResume", label: "Pause / resume recording", description: "Pauses or resumes the active recording." },
];

// Combos Windows or common apps reserve — warn (the OS won't let us own these).
const RESERVED = new Set([
  "Alt+F4", "Alt+Tab", "Super+KeyL", "Super+KeyD", "Super+Tab",
  "Super+Shift+KeyS", "Ctrl+Shift+Escape", "Ctrl+Alt+Delete", "Super+KeyR",
]);

/** Build a registerable accelerator (e.g. "Ctrl+Shift+F9") from a keydown. */
function accelFromEvent(e: KeyboardEvent): string | null {
  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Ctrl");
  if (e.shiftKey) mods.push("Shift");
  if (e.altKey) mods.push("Alt");
  if (e.metaKey) mods.push("Super");
  const code = e.code;
  // Ignore lone modifier presses.
  if (/^(Control|Shift|Alt|Meta)(Left|Right)$/.test(code)) return null;
  if (mods.length === 0) return null; // require at least one modifier
  return [...mods, code].join("+");
}

/** Human-readable form: KeyR→R, Digit1→1, Super→Win. */
function prettify(accel: string): string {
  return accel
    .split("+")
    .map((p) =>
      p === "Super"
        ? "Win"
        : p.replace(/^Key/, "").replace(/^Digit/, ""),
    )
    .join(" + ");
}

export function KeyboardShortcutsSettings() {
  const [bindings, setBindings] = useState<ShortcutBindings | null>(null);
  const [capturing, setCapturing] = useState<ActionKey | null>(null);
  const [conflicts, setConflicts] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    invoke<ShortcutBindings>("get_shortcuts")
      .then(setBindings)
      .catch(() => setBindings({ startStop: "Ctrl+Shift+F9", pauseResume: "Ctrl+Shift+F10" }));
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
      const accel = accelFromEvent(e);
      if (!accel) return;
      setBindings((prev) => (prev ? { ...prev, [capturing]: accel } : prev));
      setCapturing(null);
    };
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [capturing]);

  const save = useCallback(async () => {
    if (!bindings) return;
    setSaving(true);
    try {
      const result = await invoke<ShortcutApplyResult>("set_shortcuts", { bindings });
      setConflicts(result.conflicts);
      if (result.conflicts.length === 0) {
        toast.success("Shortcuts saved");
      } else {
        toast.warning("Some shortcuts couldn't be registered", {
          description: "They're in use by Windows or another app. Pick a different combo.",
        });
      }
    } catch (e) {
      toast.error("Failed to save shortcuts", {
        description: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setSaving(false);
    }
  }, [bindings]);

  if (!bindings) return null;

  return (
    <div className="rounded-lg border border-border bg-card p-6 shadow-sm">
      <div className="mb-4 flex items-center gap-2">
        <Keyboard className="h-5 w-5 text-primary" />
        <div>
          <h3 className="text-lg font-semibold text-foreground">Keyboard shortcuts</h3>
          <p className="mt-1 text-sm text-muted-foreground">
            Global shortcuts — they work even when ClawScribe isn&apos;t focused.
          </p>
        </div>
      </div>

      <div className="space-y-3">
        {ACTIONS.map(({ key, label, description }) => {
          const accel = bindings[key];
          const isReserved = RESERVED.has(accel);
          const conflicted = conflicts.includes(key);
          return (
            <div
              key={key}
              className="flex items-center justify-between gap-4 rounded-md border border-border bg-background p-3"
            >
              <div className="min-w-0">
                <p className="text-sm font-medium text-foreground">{label}</p>
                <p className="text-xs text-muted-foreground">{description}</p>
                {(isReserved || conflicted) && (
                  <p className="mt-1 flex items-center gap-1 text-xs text-amber-500">
                    <AlertTriangle className="h-3.5 w-3.5" />
                    {conflicted
                      ? "In use by Windows or another app — not active."
                      : "This is a reserved Windows shortcut."}
                  </p>
                )}
              </div>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => setCapturing(key)}
                className="min-w-[150px] font-mono"
              >
                {capturing === key ? "Press keys… (Esc)" : prettify(accel)}
              </Button>
            </div>
          );
        })}
      </div>

      <div className="mt-4 flex justify-end">
        <Button type="button" onClick={save} disabled={saving || capturing !== null}>
          {saving ? "Saving…" : "Save shortcuts"}
        </Button>
      </div>
    </div>
  );
}
