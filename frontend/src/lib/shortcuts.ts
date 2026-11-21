export interface ShortcutBindings {
  startStop: string;
  pauseResume: string;
  toggleWindow: string;
  bookmarkRecording: string;
}

export type ShortcutAction = keyof ShortcutBindings;

export const SHORTCUT_ACTIONS: { key: ShortcutAction; label: string }[] = [
  { key: "startStop", label: "Start or stop recording" },
  { key: "pauseResume", label: "Pause or resume recording" },
  { key: "toggleWindow", label: "Show or hide ClawScribe" },
  { key: "bookmarkRecording", label: "Bookmark this moment while recording" },
];

export const DEFAULT_BINDINGS: ShortcutBindings = {
  startStop: "Ctrl+Shift+F9",
  pauseResume: "Ctrl+Shift+F10",
  toggleWindow: "Ctrl+Shift+F11",
  bookmarkRecording: "Ctrl+Shift+F8",
};

// Combos Windows or common apps reserve. The OS will not let us own these.
export const RESERVED_SHORTCUTS = new Set([
  "Alt+F4", "Alt+Tab", "Super+KeyL", "Super+KeyD", "Super+Tab",
  "Shift+Super+KeyS", "Ctrl+Shift+Escape", "Ctrl+Alt+Delete", "Super+KeyR",
]);

export type CapturedKey =
  | { kind: "ignore" }
  | { kind: "needs-modifier" }
  | { kind: "accelerator"; accelerator: string };

export function shortcutModifiers(e: { ctrlKey?: boolean; shiftKey?: boolean; altKey?: boolean; metaKey?: boolean }): string[] {
  return [e.ctrlKey && "Ctrl", e.shiftKey && "Shift", e.altKey && "Alt", e.metaKey && "Super"].filter((part): part is string => !!part);
}

// The Windows global-hotkey registrar maps these accelerator names to virtual
// keys. The physical DOM code can differ, for example KeyZ for the German Y.
const WINDOWS_PRINTABLE_CODES: Record<number, string> = {
  186: 'Semicolon', 187: 'Equal', 188: 'Comma', 189: 'Minus', 190: 'Period',
  191: 'Slash', 192: 'Backquote', 219: 'BracketLeft', 220: 'Backslash', 221: 'BracketRight', 222: 'Quote',
};
for (let vk = 65; vk <= 90; vk++) WINDOWS_PRINTABLE_CODES[vk] = 'Key' + String.fromCharCode(vk);
for (let vk = 48; vk <= 57; vk++) WINDOWS_PRINTABLE_CODES[vk] = 'Digit' + String.fromCharCode(vk);

export function printableVirtualKey(code: string): number | undefined {
  const match = Object.entries(WINDOWS_PRINTABLE_CODES).find(([, name]) => name === code);
  return match ? Number(match[0]) : undefined;
}

/** Build a registerable accelerator (for example "Ctrl+Shift+F9") from a keydown. */
export function captureAccelerator(e: Pick<KeyboardEvent, "ctrlKey" | "shiftKey" | "altKey" | "metaKey" | "code"> & { keyCode?: number }): CapturedKey {
  // A lone modifier press is not a combination yet.
  if (/^(Control|Shift|Alt|Meta)(Left|Right)$/.test(e.code)) return { kind: "ignore" };
  const mods = shortcutModifiers(e);
  if (mods.length === 0) return { kind: "needs-modifier" };
  const windows = typeof window !== "undefined" && window.navigator?.platform.startsWith("Win");
  const code = windows && e.keyCode ? WINDOWS_PRINTABLE_CODES[e.keyCode] ?? e.code : e.code;
  return { kind: "accelerator", accelerator: [...mods, code].join("+") };
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
