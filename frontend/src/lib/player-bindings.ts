import { invoke } from '@tauri-apps/api/core';
import { shortcutModifiers } from './shortcuts';
import type { PlaybackShortcut, ShortcutEvent } from './playback-controls';

export const DEFAULT_PLAYER_BINDINGS = {
  toggle: 'Space', back: 'ArrowLeft', forward: 'ArrowRight',
  backLarge: 'Shift+ArrowLeft', forwardLarge: 'Shift+ArrowRight',
  slower: '<', faster: '>', follow: 'F', bookmark: 'B', find: 'Ctrl+F',
};
export type PlayerAction = keyof typeof DEFAULT_PLAYER_BINDINGS;
export type PlayerBindings = Record<PlayerAction, string>;

export const PLAYER_ACTIONS: { key: PlayerAction; label: string; command: PlaybackShortcut }[] = [
  { key: 'toggle', label: 'Play or pause', command: { type: 'toggle' } },
  { key: 'back', label: 'Back 5 seconds', command: { type: 'seek', seconds: -5 } },
  { key: 'forward', label: 'Forward 5 seconds', command: { type: 'seek', seconds: 5 } },
  { key: 'backLarge', label: 'Back 15 seconds', command: { type: 'seek', seconds: -15 } },
  { key: 'forwardLarge', label: 'Forward 15 seconds', command: { type: 'seek', seconds: 15 } },
  { key: 'slower', label: 'Slower playback', command: { type: 'rate', direction: -1 } },
  { key: 'faster', label: 'Faster playback', command: { type: 'rate', direction: 1 } },
  { key: 'follow', label: 'Follow playback in the transcript', command: { type: 'follow' } },
  { key: 'bookmark', label: 'Add a bookmark at this moment', command: { type: 'bookmark' } },
  { key: 'find', label: 'Find in the focused pane', command: { type: 'find' } },
];

export const PLAYER_BINDINGS_STORAGE = 'clawscribe.playerShortcuts.v1';

let nativeBindings: PlayerBindings | null = null;
let loading: Promise<PlayerBindings> | null = null;
let savedRevision = 0;

function readBindings(value: unknown): Partial<PlayerBindings> {
  const bindings: Partial<PlayerBindings> = {};
  if (!value || typeof value !== 'object') return bindings;
  const source = value as Record<string, unknown>;
  for (const { key } of PLAYER_ACTIONS) {
    const chord = source[key];
    if (typeof chord === 'string' && chord.trim()) bindings[key] = chord;
  }
  return bindings;
}

function readLegacyBindings(): Partial<PlayerBindings> | null {
  try {
    const raw = window.localStorage.getItem(PLAYER_BINDINGS_STORAGE);
    return raw ? readBindings(JSON.parse(raw)) : null;
  } catch { return null; }
}

export function loadPlayerBindings(): PlayerBindings {
  return nativeBindings ?? { ...DEFAULT_PLAYER_BINDINGS, ...readLegacyBindings() };
}

export function refreshPlayerBindings(): Promise<PlayerBindings> {
  const revision = savedRevision;
  loading ??= invoke<unknown>('load_player_shortcuts', { legacyBindings: readLegacyBindings() })
    .then(bindings => {
      if (revision === savedRevision) nativeBindings = { ...DEFAULT_PLAYER_BINDINGS, ...readBindings(bindings) };
      return loadPlayerBindings();
    })
    .catch(loadPlayerBindings)
    .finally(() => { loading = null; });
  return loading;
}

export async function savePlayerBindings(bindings: PlayerBindings): Promise<void> {
  await invoke('set_player_shortcuts', { bindings });
  savedRevision += 1;
  nativeBindings = bindings;
  try {
    window.localStorage.setItem(PLAYER_BINDINGS_STORAGE, JSON.stringify(bindings));
  } catch { /* This copy is for older builds. The native save already succeeded. */ }
  window.dispatchEvent(new Event(PLAYER_BINDINGS_STORAGE));
}

const IGNORE_KEYS = new Set(['Control', 'Shift', 'Alt', 'Meta', 'AltGraph', 'OS', 'Dead', 'Process', 'Unidentified', 'CapsLock', 'NumLock', 'ScrollLock']);

type ChordEvent = Pick<ShortcutEvent, 'key' | 'ctrlKey' | 'shiftKey' | 'altKey' | 'metaKey' | 'isComposing'>;

/** Playback uses the character printed on this keyboard, even when its physical code differs. */
export function playerChord(event: ChordEvent): string | null {
  const { key, ctrlKey, shiftKey, altKey, metaKey } = event;
  if (!key || IGNORE_KEYS.has(key) || event.isComposing) return null;
  const upper = key.toUpperCase();
  let printed = upper.length === 1 ? upper : key;
  if (key === ' ') printed = 'Space';
  if (key === '+') printed = 'Plus';
  // Angle brackets use Shift on some layouts and a dedicated key on others.
  const angle = key === '<' || key === '>';
  return [...shortcutModifiers({ ctrlKey, shiftKey: shiftKey && !angle, altKey, metaKey }), printed].join('+');
}

export function playerActionForChord(bindings: PlayerBindings, chord: string) {
  const direct = PLAYER_ACTIONS.find(({ key }) => bindings[key] === chord);
  if (direct) return direct;
  // Older versions also accepted Command+F, Shift+F/B and Shift+Space.
  const alias = ({ 'Super+F': 'find', 'Shift+F': 'follow', 'Shift+B': 'bookmark', 'Shift+Space': 'toggle' } as Record<string, PlayerAction>)[chord];
  if (alias && bindings[alias] === DEFAULT_PLAYER_BINDINGS[alias]) return PLAYER_ACTIONS.find(({ key }) => key === alias);
}

export function playerKeycaps(chord: string): string[] {
  const display: Record<string, string> = { Super: 'Win', Plus: '+', ArrowLeft: '←', ArrowRight: '→', ArrowUp: '↑', ArrowDown: '↓' };
  return chord.split('+').map(key => display[key] ?? key);
}

export function ariaPlayerChord(chord: string): string {
  return chord.split('+').map(key => key === 'Ctrl' ? 'Control' : key === 'Super' ? 'Meta' : key).join('+');
}

/** Commands implemented by AppShortcuts and the saved transcript/summary panels. */
export function fixedShortcutOwner(event: ChordEvent): string | undefined {
  if (event.key === 'Tab') return 'keyboard navigation';
  if (!(event.ctrlKey || event.metaKey)) return;
  if (event.key.toLowerCase() === 'h') return 'Find and replace';
  if (!event.altKey) {
    return ({ ',': 'Open settings', k: 'Search meetings', g: 'Generate summary' } as Record<string, string>)[event.key.toLowerCase()];
  }
}
