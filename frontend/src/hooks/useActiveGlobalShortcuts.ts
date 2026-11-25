import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { ShortcutAction } from '@/lib/shortcuts';

export function useActiveGlobalShortcuts() {
  const [shortcuts, setShortcuts] = useState<Partial<Record<ShortcutAction, string>>>({});
  useEffect(() => {
    let disposed = false;
    let version = 0;
    const refresh = async () => {
      const request = ++version;
      try {
        const registered = await invoke<[ShortcutAction, string][]>('get_active_shortcuts');
        if (!disposed && request === version) setShortcuts(Object.fromEntries(registered));
      } catch {
        if (!disposed && request === version) setShortcuts({});
      }
    };
    const subscription = listen('global-shortcuts-changed', refresh);
    void subscription.then(refresh, refresh);
    return () => {
      disposed = true;
      void subscription.then(unlisten => unlisten(), () => {});
    };
  }, []);
  return shortcuts;
}
