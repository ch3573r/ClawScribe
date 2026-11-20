'use client';

import { useEffect } from 'react';
import { toast } from 'sonner';
import { recordingService, type ReconnectionStatus } from '@/services/recordingService';
import { useRecordingState } from '@/contexts/RecordingStateContext';

type Device = ReconnectionStatus['disconnected_devices'][number];
const deviceTypes = ['Microphone', 'SystemAudio'] as const;
const label = (type: Device['device_type']) => type === 'Microphone' ? 'Microphone' : 'System audio';
const toastId = (type: Device['device_type']) => `audio-device-hot-swap-${type}`;

export function AudioDeviceHotSwapMonitor() {
  const { isRecording, sessionId } = useRecordingState();
  useEffect(() => {
    if (!isRecording || !sessionId) return;
    let disposed = false;
    let polling = false;
    let reconnecting = false;
    const visible = new Set<Device['device_type']>();

    const showRetry = (device: Device, detail?: string) => {
      visible.add(device.device_type);
      toast.warning(`${label(device.device_type)} unavailable`, {
        id: toastId(device.device_type),
        description: detail ?? `${device.name} is missing from this recording. Try reconnecting.`,
        action: { label: 'Retry', onClick: () => { void reconnect(device); } },
        duration: Infinity,
      });
    };

    const reconnect = async (device: Device) => {
      if (disposed || reconnecting) return;
      reconnecting = true;
      toast.loading(`Reconnecting ${label(device.device_type).toLowerCase()}...`, {
        id: toastId(device.device_type), description: device.name,
      });
      try {
        const result = await recordingService.attemptDeviceReconnect(device.name, device.device_type, sessionId);
        if (disposed) return;
        if (result.reconnected) {
          visible.delete(device.device_type);
          toast.success(`${label(device.device_type)} reconnected`, {
            id: toastId(device.device_type), description: device.name,
          });
        } else {
          const active = device.device_type === 'SystemAudio' ? result.microphone_active : result.system_audio_active;
          const other = device.device_type === 'SystemAudio' ? 'Microphone' : 'System audio';
          showRetry(device, active ? `${other} capture continues. Retry the missing source.` : undefined);
        }
      } catch (error) {
        if (!disposed) showRetry(device, String(error));
      } finally {
        reconnecting = false;
      }
    };

    const poll = async () => {
      if (disposed || polling || reconnecting) return;
      polling = true;
      try {
        const event = await recordingService.pollAudioDeviceEvents();
        if (disposed) return;
        const status = await recordingService.getReconnectionStatus();
        if (disposed || status.is_reconnecting || reconnecting) return;
        const pending = new Map(status.disconnected_devices.map(device => [device.device_type, device]));
        for (const type of visible) {
          if (!pending.has(type)) {
            toast.dismiss(toastId(type));
            visible.delete(type);
          }
        }
        for (const device of pending.values()) {
          if (!visible.has(device.device_type)) showRetry(device);
        }
        if (event?.type === 'DeviceReconnected') {
          const device = pending.get(event.device_type);
          if (device?.name === event.device_name) await reconnect(device);
        }
      } catch {
        if (!disposed) console.warn('Could not check recording audio devices.');
      } finally {
        polling = false;
      }
    };

    void poll();
    const interval = window.setInterval(() => { void poll(); }, 1500);
    return () => {
      disposed = true;
      window.clearInterval(interval);
      for (const type of deviceTypes) toast.dismiss(toastId(type));
    };
  }, [isRecording, sessionId]);

  return null;
}
