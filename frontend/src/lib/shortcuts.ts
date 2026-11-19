export interface ShortcutBindings {
  startStop: string;
  pauseResume: string;
  toggleWindow: string;
}

export type ShortcutAction = keyof ShortcutBindings;

export const SHORTCUT_ACTIONS: { key: ShortcutAction; label: string }[] = [
  { key: "startStop", label: "Start or stop recording" },
  { key: "pauseResume", label: "Pause or resume recording" },
  { key: "toggleWindow", label: "Show or hide ClawScribe" },
];

export const DEFAULT_BINDINGS: ShortcutBindings = {
  startStop: "Ctrl+Shift+F9",
  pauseResume: "Ctrl+Shift+F10",
  toggleWindow: "Ctrl+Shift+F11",
};

// Combos Windows or common apps reserve. The OS will not let us own these.
export const RESERVED_SHORTCUTS = new Set([
  "Alt+F4", "Alt+Tab", "Super+KeyL", "Super+KeyD", "Super+Tab",
  "Super+Shift+KeyS", "Ctrl+Shift+Escape", "Ctrl+Alt+Delete", "Super+KeyR",
]);

export type CapturedKey =
  | { kind: "ignore" }
  | { kind: "needs-modifier" }
  | { kind: "accelerator"; accelerator: string };

/** Build a registerable accelerator (for example "Ctrl+Shift+F9") from a keydown. */
export function captureAccelerator(e: Pick<KeyboardEvent, "ctrlKey" | "shiftKey" | "altKey" | "metaKey" | "code">): CapturedKey {
  // A lone modifier press is not a combination yet.
  if (/^(Control|Shift|Alt|Meta)(Left|Right)$/.test(e.code)) return { kind: "ignore" };
  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Ctrl");
  if (e.shiftKey) mods.push("Shift");
  if (e.altKey) mods.push("Alt");
  if (e.metaKey) mods.push("Super");
  if (mods.length === 0) return { kind: "needs-modifier" };
  return { kind: "accelerator", accelerator: [...mods, e.code].join("+") };
}

/** Separate keycaps for a shortcut: "Super+KeyR" becomes Win, R. */
export function keycaps(accelerator: string): string[] {
  return accelerator.split("+").map((part) =>
    part === "Super" ? "Win" : part.replace(/^Key/, "").replace(/^Digit/, ""),
  );
}

/** The action that already uses this combination, if any other action does. */
export function duplicateAction(bindings: ShortcutBindings, action: ShortcutAction, accelerator: string): ShortcutAction | null {
  return (Object.keys(bindings) as ShortcutAction[]).find((key) => key !== action && bindings[key] === accelerator) ?? null;
}

export function isDefaultBindings(bindings: ShortcutBindings): boolean {
  return (Object.keys(DEFAULT_BINDINGS) as ShortcutAction[]).every((key) => bindings[key] === DEFAULT_BINDINGS[key]);
}
