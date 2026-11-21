import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { selectedDevicesFromPreferences, type AudioDevicePreferenceFields, type SelectedDevices } from '@/lib/audioDevicePreferences';
import { configService } from '@/services/configService';

export function useRecordingDevicePreferences() {
  const [selectedDevices, setSelectedDevices] = useState<SelectedDevices>({ micDevice: null, systemDevice: null });
  const [devicePreferencesStatus, setStatus] = useState<'loading' | 'ready' | 'error'>('loading');
  const [isSavingDevicePreferences, setIsSaving] = useState(false);
  const revision = useRef(0);
  const saving = useRef(false);

  const apply = useCallback((preferences: AudioDevicePreferenceFields) => {
    revision.current += 1;
    setSelectedDevices(selectedDevicesFromPreferences(preferences));
    setStatus('ready');
  }, []);

  const refreshDevicePreferences = useCallback(async () => {
    const request = ++revision.current;
    setStatus('loading');
    try {
      const preferences = await configService.getRecordingPreferences();
      if (request === revision.current) apply(preferences);
    } catch {
      if (request === revision.current) setStatus('error');
    }
  }, [apply]);

  useEffect(() => {
    let active = true;
    const subscription = listen<AudioDevicePreferenceFields>('recording-device-preferences-changed', ({ payload }) => {
      if (active) apply(payload);
    });
    void subscription.catch(() => { console.warn('Could not subscribe to audio device preference changes.'); })
      .then(() => { if (active) void refreshDevicePreferences(); });
    return () => {
      active = false;
      revision.current += 1;
      void subscription.then(unlisten => unlisten()).catch(() => {});
    };
  }, [apply, refreshDevicePreferences]);

  const saveSelectedDevices = useCallback(async (devices: SelectedDevices) => {
    if (saving.current) return;
    saving.current = true;
    setIsSaving(true);
    const request = ++revision.current;
    try {
      const preferences = await invoke<AudioDevicePreferenceFields>('set_recording_device_preferences', {
        preferences: { preferred_mic_device: devices.micDevice, preferred_system_device: devices.systemDevice },
      });
      if (request === revision.current) apply(preferences);
    } catch {
      toast.error('Could not save audio device selection', { description: 'Choose the device again to retry.' });
    } finally {
      saving.current = false;
      setIsSaving(false);
    }
  }, [apply]);

  return { selectedDevices, devicePreferencesStatus, isSavingDevicePreferences, refreshDevicePreferences, saveSelectedDevices };
}
