import { DEFAULT_PLAYER_BINDINGS, playerActionForChord, playerChord, type PlayerBindings } from './player-bindings';

/**
 * Playback speed and keyboard shortcuts for the saved-meeting audio player.
 * Pure helpers: no DOM access beyond reading the event target, so the rules
 * that keep shortcuts from stealing typing can be tested without a WebView.
 */

export const PLAYBACK_RATES = [0.5, 0.75, 1, 1.25, 1.5, 1.75, 2] as const;
export const DEFAULT_PLAYBACK_RATE = 1;
export const PLAYBACK_RATE_STORAGE_KEY = 'clawscribe.playbackRate';

/** The nearest supported speed; anything unusable falls back to normal speed. */
export function normalizePlaybackRate(value: unknown): number {
  const number = typeof value === 'string' ? Number(value) : value;
  if (typeof number !== 'number' || !Number.isFinite(number) || number <= 0) return DEFAULT_PLAYBACK_RATE;
  return PLAYBACK_RATES.reduce((best, rate) => Math.abs(rate - number) < Math.abs(best - number) ? rate : best, PLAYBACK_RATES[0]) as number;
}

/** One supported step faster (+1) or slower (-1), stopping at the ends. */
export function stepPlaybackRate(current: number, direction: 1 | -1): number {
  const index = PLAYBACK_RATES.indexOf(normalizePlaybackRate(current) as (typeof PLAYBACK_RATES)[number]);
  return PLAYBACK_RATES[Math.max(0, Math.min(PLAYBACK_RATES.length - 1, index + direction))];
}

export function formatPlaybackRate(rate: number): string {
  return `${Number(rate.toFixed(2))}×`;
}

export type PlaybackShortcut =
  | { type: 'toggle' }
  | { type: 'seek'; seconds: number }
  | { type: 'rate'; direction: 1 | -1 }
  | { type: 'follow' }
  | { type: 'bookmark' }
  | { type: 'find' };

export interface ShortcutTarget {
  tagName?: string;
  isContentEditable?: boolean;
  getAttribute?: (name: string) => string | null;
  closest?: (selector: string) => unknown;
}

export interface ShortcutEvent {
  key: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
  repeat?: boolean;
  isComposing?: boolean;
  defaultPrevented?: boolean;
  target?: ShortcutTarget | null;
}

const TEXT_ENTRY = new Set(['INPUT', 'TEXTAREA', 'SELECT']);
// Controls that own Space and Enter.
const ACTIVATES_ON_SPACE = new Set(['BUTTON', 'A', 'SUMMARY']);
// Controls that own the arrow keys.
const OWNS_ARROWS = new Set(['slider', 'tab', 'radio', 'menuitem', 'menuitemradio', 'menuitemcheckbox', 'option', 'combobox', 'listbox', 'spinbutton', 'tree', 'treeitem', 'grid', 'gridcell', 'scrollbar', 'tablist', 'menu', 'menubar']);
const OWNS_SPACE = new Set([...OWNS_ARROWS, 'button', 'switch', 'checkbox', 'link']);
const MODAL_SURFACE = '[role="dialog"],[role="alertdialog"],[role="menu"],[role="listbox"],[data-radix-popper-content-wrapper]';

function role(target: ShortcutTarget): string {
  return target.getAttribute?.('role')?.toLowerCase() ?? '';
}

/** True while the user is typing, so a shortcut must not fire. */
export function isTextEntry(target: ShortcutTarget | null | undefined): boolean {
  if (!target) return false;
  if (target.isContentEditable) return true;
  if (TEXT_ENTRY.has(target.tagName?.toUpperCase() ?? '')) return true;
  if (['textbox', 'searchbox', 'combobox'].includes(role(target))) return true;
  return Boolean(target.closest?.('[contenteditable=""],[contenteditable="true"],[role="textbox"]'));
}

/** Leave typing, menus, dialogs and the focused control's own navigation keys alone. */
export function resolvePlaybackShortcut(event: ShortcutEvent, bindings: PlayerBindings = DEFAULT_PLAYER_BINDINGS): PlaybackShortcut | null {
  if (event.defaultPrevented || event.isComposing) return null;
  const target = event.target;
  if (target && (isTextEntry(target) || target.closest?.(MODAL_SURFACE))) return null;
  const tag = target?.tagName?.toUpperCase() ?? '';
  const targetRole = target ? role(target) : '';
  if ((event.key === ' ' || event.key === 'Enter') && (ACTIVATES_ON_SPACE.has(tag) || OWNS_SPACE.has(targetRole))) return null;
  if (/^(Arrow(Left|Right|Up|Down)|Home|End|PageUp|PageDown)$/.test(event.key) && OWNS_ARROWS.has(targetRole)) return null;

  const chord = playerChord(event);
  const command = chord && playerActionForChord(bindings, chord)?.command;
  if (!command || (event.repeat && command.type !== 'seek' && command.type !== 'rate')) return null;
  return command;
}
