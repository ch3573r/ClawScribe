import { useEffect, useRef } from 'react';
import { resolvePlaybackShortcut, type PlaybackShortcut, type ShortcutEvent } from '@/lib/playback-controls';

/**
 * Keyboard control of the saved-meeting player while the meeting page is open.
 * Every key is resolved by `resolvePlaybackShortcut`, which stands down while
 * the user types, inside dialogs and menus, and on controls that own the key.
 * Handlers are read through a ref, so the listener is attached once.
 */
export function usePlaybackShortcuts(handlers: { [K in PlaybackShortcut['type']]?: (shortcut: Extract<PlaybackShortcut, { type: K }>) => void }, enabled = true) {
  const latest = useRef(handlers);
  latest.current = handlers;
  useEffect(() => {
    if (!enabled) return;
    const onKeyDown = (event: KeyboardEvent) => {
      const shortcut = resolvePlaybackShortcut(event as unknown as ShortcutEvent);
      if (!shortcut) return;
      const handler = latest.current[shortcut.type] as ((value: PlaybackShortcut) => void) | undefined;
      if (!handler) return;
      event.preventDefault();
      handler(shortcut);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [enabled]);
}
