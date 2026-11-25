"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { AlertTriangle, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Kbd, SettingsCard, SettingsFooter, SettingsRow, SettingsStack } from "@/components/ui/settings-layout";
import { NoteRow } from "@/components/settings/settings-parts";
import {
  captureAccelerator,
  DEFAULT_BINDINGS,
  isDefaultBindings,
  keycaps,
  printableVirtualKey,
  RESERVED_SHORTCUTS,
  SHORTCUT_ACTIONS,
  type ShortcutAction,
  type ShortcutBindings,
} from "@/lib/shortcuts";

import {
  DEFAULT_PLAYER_BINDINGS, fixedShortcutOwner, PLAYER_ACTIONS, playerActionForChord, playerChord,
  playerKeycaps, savePlayerBindings, type PlayerAction, type PlayerBindings,
} from "@/lib/player-bindings";
import { usePlayerBindingState } from "@/hooks/usePlaybackShortcuts";

type Action = ShortcutAction | PlayerAction;
type Group = "global" | "player";
const globalLabel = (key: ShortcutAction) => SHORTCUT_ACTIONS.find(action => action.key === key)!.label;
const taken = (owner: string) => `That combination is already used for "${owner}". Pick a different one.`;

interface ShortcutApplyResult {
  bindings: ShortcutBindings;
  conflicts: string[];
}

const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));

// Keep releases ordered with subsequent starts even when the editor unmounts
// before its IPC call completes. Recover the queue after an IPC failure.
let captureQueue: Promise<unknown> = Promise.resolve();
function setNativeCapture(channel: Channel<ShortcutAction> | null) {
  const result = captureQueue.then(() => invoke<number>("set_shortcut_capture", { channel }));
  captureQueue = result.catch(() => {});
  return result;
}

async function playerChordsForGlobals(globals: ShortcutBindings) {
  const parsed = SHORTCUT_ACTIONS.map(({ key, label }) => {
    const parts = globals[key].split("+");
    const code = parts.pop()!;
    return { label, code, vk: printableVirtualKey(code), shiftKey: parts.includes("Shift"),
      ctrlKey: parts.includes("Ctrl"), altKey: parts.includes("Alt"), metaKey: parts.includes("Super") };
  });
  const printable = parsed.filter(item => item.vk !== undefined);
  const keys = printable.map(({ vk, shiftKey, ctrlKey, altKey }) => [vk, shiftKey, ctrlKey && altKey]);
  const characters = keys.length ? await invoke<(string | null)[]>("shortcut_key_characters", { keys }) : [];
  return parsed.map(item => {
    const index = printable.indexOf(item);
    const key = characters[index] ?? item.code.replace(/^Key|^Digit/, "");
    return { label: item.label, chord: playerChord({ ...item, key }) };
  });
}

export function KeyboardShortcutsSettings() {
  const [bindings, setBindings] = useState<ShortcutBindings | null>(null);
  const [capturing, setCapturing] = useState<Action | null>(null);
  const { bindings: player, loading: playerLoading } = usePlayerBindingState();
  const [group, setGroup] = useState<Group>("global");
  const [ready, setReady] = useState(false);
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

  const savePlayer = useCallback(async (next: PlayerBindings) => {
    setApplying("all");
    try {
      await savePlayerBindings(next);
      setError(null);
      setNotice("Player shortcuts updated");
    } catch {
      setError("Could not save the player shortcuts. Try again.");
    } finally {
      setApplying(null);
    }
  }, []);

  useEffect(() => {
    if (!capturing) return;
    let active = true;
    let timeout: ReturnType<typeof setTimeout>;
    const started = Date.now();
    const channel = new Channel<ShortcutAction>();
    channel.onmessage = owner => {
      if (!active) return;
      setError(owner === capturing ? null : taken(globalLabel(owner)));
      setCapturing(null);
    };
    const handleKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const cancel = () => setCapturing(null);
      if (e.key === "Escape") { cancel(); return; }
      if (e.repeat || e.isComposing) return;
      const current = bindingsRef.current;
      const chord = playerChord(e);
      if (!current || !chord) return;
      const native = captureAccelerator(e);
      if (native.kind === "ignore") return;
      const editingGlobal = capturing in DEFAULT_BINDINGS;
      if (editingGlobal && native.kind === "needs-modifier") {
        setError("Press Ctrl, Alt, Shift or Win together with another key.");
        return;
      }
      cancel();
      const reserved = native.kind === "accelerator" && RESERVED_SHORTCUTS.has(native.accelerator);
      if (reserved) { setError("Windows reserves that shortcut. Pick a different one."); return; }
      const fixed = fixedShortcutOwner(e);
      if (fixed) { setError(taken(fixed)); return; }
      const otherPlayer = playerActionForChord(player, chord);
      if (otherPlayer && otherPlayer.key !== capturing) { setError(taken(otherPlayer.label)); return; }
      if (native.kind === "accelerator") {
        const otherGlobal = SHORTCUT_ACTIONS.find(({ key }) => key !== capturing && current[key] === native.accelerator);
        if (otherGlobal) { setError(taken(otherGlobal.label)); return; }
        if (editingGlobal) {
          if (current[capturing as ShortcutAction] !== native.accelerator) {
            void apply({ ...current, [capturing]: native.accelerator }, capturing as ShortcutAction);
          }
          return;
        }
      }
      void savePlayer({ ...player, [capturing]: chord });
    };
    const blur = () => setCapturing(null);
    // Keep recording hotkeys registered. While this editor owns capture the native
    // handler reports an occupied shortcut through the channel instead of firing it.
    void setNativeCapture(channel).then(duration => {
      if (!active) return;
      setReady(true);
      window.addEventListener("keydown", handleKey, true);
      timeout = setTimeout(blur, Math.max(0, duration - (Date.now() - started) - 250));
    }).catch(() => {
      if (active) {
        setError("Could not start shortcut capture. Try again.");
        setCapturing(null);
      }
    });
    window.addEventListener("blur", blur);
    return () => {
      active = false;
      clearTimeout(timeout);
      window.removeEventListener("keydown", handleKey, true);
      window.removeEventListener("blur", blur);
      void setNativeCapture(null).catch(() => {});
    };
  }, [capturing, player, apply, savePlayer]);

  if (!bindings) {
    return <SettingsStack><SettingsCard><SettingsFooter>
      <span role="status" className="inline-flex items-center gap-2"><Loader2 aria-hidden="true" className="h-3.5 w-3.5 animate-spin" />Loading shortcuts…</span>
    </SettingsFooter></SettingsCard></SettingsStack>;
  }

  const busy = applying !== null || playerLoading;
  const reset = async (scope: Group) => {
    setGroup(scope);
    setError(null);
    setNotice("");
    // Each group resets independently. Avoid taking a shortcut customized in the other group.
    const nextGlobal = scope === "global" ? DEFAULT_BINDINGS : bindings;
    const nextPlayer = scope === "player" ? DEFAULT_PLAYER_BINDINGS : player;
    setApplying("all");
    try {
      for (const { label, chord } of await playerChordsForGlobals(nextGlobal)) {
        const other = chord && playerActionForChord(nextPlayer, chord);
        if (other) { setError(taken(scope === "player" ? label : other.label)); return; }
      }
      if (scope === "player") await savePlayer({ ...DEFAULT_PLAYER_BINDINGS });
      else await apply({ ...DEFAULT_BINDINGS }, "all");
    } catch {
      setError("Could not check the current keyboard layout. Try again.");
    } finally {
      setApplying(null);
    }
  };

  const card = (scope: Group) => {
    const isPlayer = scope === "player";
    const actions = isPlayer ? PLAYER_ACTIONS : SHORTCUT_ACTIONS;
    const defaults = isPlayer ? PLAYER_ACTIONS.every(({ key }) => player[key] === DEFAULT_PLAYER_BINDINGS[key]) : isDefaultBindings(bindings);
    return <SettingsCard title={isPlayer ? "Saved meeting player" : "Global shortcuts"}
      description={isPlayer ? "Active while a saved meeting is open." : "Active even when ClawScribe is in the background."}>
      {actions.map(({ key, label }) => {
        const caps = isPlayer ? playerKeycaps(player[key as PlayerAction]) : keycaps(bindings[key as ShortcutAction]);
        const isCapturing = capturing === key;
        const conflicted = !isPlayer && conflicts.includes(key);
        const reserved = !isPlayer && RESERVED_SHORTCUTS.has(bindings[key as ShortcutAction]);
        return <SettingsRow key={key} title={label} description={(reserved || conflicted) && (
          <span className="inline-flex items-center gap-1.5 text-warning-foreground">
            <AlertTriangle aria-hidden="true" className="h-3.5 w-3.5 shrink-0" />
            {conflicted ? "In use by Windows or another app" : "Windows reserves this shortcut"}
          </span>
        )}>
          <Button type="button" variant="ghost" size="sm" disabled={busy || readFailed}
            aria-label={isCapturing ? `${label}, press the new keys or Escape to cancel` : `${label}, ${caps.join(" + ")}. Change shortcut`}
            onClick={() => { setGroup(scope); setReady(false); setError(null); setNotice(""); setCapturing(isCapturing ? null : key); }}
            onBlur={() => setCapturing(current => current === key ? null : current)}
            className="min-h-[34px] gap-1.5 px-1.5">
            {isCapturing ? <span className="text-primary">{ready ? "Press the new keys · Esc to cancel" : "Starting capture…"}</span>
              : applying === key ? <Loader2 aria-hidden="true" className="h-4 w-4 animate-spin" />
              : caps.map((cap, index) => <Kbd key={index}>{cap}</Kbd>)}
          </Button>
        </SettingsRow>;
      })}
      {!isPlayer && readFailed && <NoteRow tone="warning">
        The saved global shortcuts could not be read.{" "}
        <Button variant="link" size="sm" className="h-auto p-0" onClick={load}>Try again</Button>
      </NoteRow>}
      {error && group === scope && <NoteRow tone="error">{error}</NoteRow>}
      <SettingsFooter actions={<Button type="button" variant="ghost" size="sm"
        disabled={busy || capturing !== null || (isPlayer && readFailed) || (defaults && !readFailed)}
        onClick={() => reset(scope)}>
        {isPlayer ? "Reset player shortcuts" : "Reset global shortcuts"}
      </Button>}>
        {isPlayer ? "Click a shortcut and press a key or combination." : "Click a shortcut and press a key with Ctrl, Alt, Shift or Win."}
      </SettingsFooter>
    </SettingsCard>;
  };
  return <SettingsStack>{card("global")}{card("player")}<span role="status" className="sr-only">{notice}</span></SettingsStack>;
}
