import { useEffect, useRef, useState } from 'react';
import { resolvePlaybackShortcut, type PlaybackShortcut, type ShortcutEvent } from '@/lib/playback-controls';
import { loadPlayerBindings, PLAYER_BINDINGS_STORAGE } from '@/lib/player-bindings';

export function usePlayerBindings() {
  const [bindings, setBindings] = useState(loadPlayerBindings);
  useEffect(() => {
    const reload = () => setBindings(loadPlayerBindings());
    const onStorage = (event: StorageEvent) => {
      if (!event.key || event.key === PLAYER_BINDINGS_STORAGE) reload();
    };
    window.addEventListener('storage', onStorage);
    window.addEventListener(PLAYER_BINDINGS_STORAGE, reload);
    return () => {
      window.removeEventListener('storage', onStorage);
      window.removeEventListener(PLAYER_BINDINGS_STORAGE, reload);
    };
  }, []);
  return bindings;
}

/**
 * Keyboard control of the saved-meeting player while the meeting page is open.
 * Every key is resolved by `resolvePlaybackShortcut`, which stands down while
 * the user types, inside dialogs and menus, and on controls that own the key.
 * Handlers are read through a ref, so the listener is attached once.
 */
export function usePlaybackShortcuts(handlers: { [K in PlaybackShortcut['type']]?: (shortcut: Extract<PlaybackShortcut, { type: K }>) => void }, enabled = true) {
  const bindings = usePlayerBindings();
  const latest = useRef({ handlers, bindings });
  latest.current = { handlers, bindings };
  useEffect(() => {
    if (!enabled) return;
    const onKeyDown = (event: KeyboardEvent) => {
      const shortcut = resolvePlaybackShortcut(event as unknown as ShortcutEvent, latest.current.bindings);
      if (!shortcut) return;
      const handler = latest.current.handlers[shortcut.type] as ((value: PlaybackShortcut) => void) | undefined;
      if (!handler) return;
      event.preventDefault();
      handler(shortcut);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [enabled]);
  return bindings;
}
