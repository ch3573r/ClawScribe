import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { AudioDeviceInventory } from '@/lib/audioDevicePreferences';

type InventoryState =
  | { status: 'checking' | 'error'; data: null }
  | { status: 'ready'; data: AudioDeviceInventory };

export function useAudioDeviceInventory() {
  const [inventory, setInventory] = useState<InventoryState>({ status: 'checking', data: null });
  const revision = useRef(0);
  const refresh = useCallback(async () => {
    const request = ++revision.current;
    try {
      const data = await invoke<AudioDeviceInventory>('get_recording_device_inventory');
      if (request === revision.current) setInventory({ status: 'ready', data });
    } catch {
      if (request === revision.current) setInventory({ status: 'error', data: null });
    }
  }, []);

  useEffect(() => {
    const recheck = () => { void refresh(); };
    window.addEventListener('focus', recheck);
    const media = navigator.mediaDevices;
    media?.addEventListener('devicechange', recheck);
    return () => {
      revision.current += 1;
      window.removeEventListener('focus', recheck);
      media?.removeEventListener('devicechange', recheck);
    };
  }, [refresh]);

  return useMemo(() => ({ ...inventory, refresh }), [inventory, refresh]);
}
