'use client';

import { useCallback, useEffect, useRef } from 'react';
import { toast } from 'sonner';
import { recordingService, ReconnectionStatus } from '@/services/recordingService';
import { useRecordingState } from '@/contexts/RecordingStateContext';

type DeviceType = 'Microphone' | 'SystemAudio';

interface DisconnectedDevice {
  name: string;
  deviceType: DeviceType;
}

const HOT_SWAP_TOAST_ID = 'audio-device-hot-swap';

function labelForDeviceType(deviceType: DeviceType): string {
  return deviceType === 'Microphone' ? 'Microphone' : 'System audio';
}

function deviceFromStatus(status: ReconnectionStatus): DisconnectedDevice | null {
  const device = status.disconnected_device;
  if (!device) return null;
  return {
    name: device.name,
    deviceType: device.device_type,
  };
}

export function AudioDeviceHotSwapMonitor() {
  const { isRecording } = useRecordingState();
  const reconnectingRef = useRef(false);
  const activeDeviceRef = useRef<DisconnectedDevice | null>(null);
  const retryReconnectRef = useRef<(device: DisconnectedDevice) => void>(() => {});

  const showDisconnectedToast = useCallback((device: DisconnectedDevice) => {
    activeDeviceRef.current = device;
    toast.warning(`${labelForDeviceType(device.deviceType)} disconnected`, {
      id: HOT_SWAP_TOAST_ID,
      description: `${device.name} is not available. Recording stays open while ClawScribe waits for the device.`,
      action: {
        label: 'Retry',
        onClick: () => {
          retryReconnectRef.current(device);
        },
      },
      duration: Infinity,
    });
  }, []);

  const retryReconnect = useCallback(async (device: DisconnectedDevice) => {
    if (reconnectingRef.current) return;
    reconnectingRef.current = true;

    toast.loading(`Reconnecting ${labelForDeviceType(device.deviceType).toLowerCase()}...`, {
      id: HOT_SWAP_TOAST_ID,
      description: device.name,
      duration: Infinity,
    });

    try {
      const connected = await recordingService.attemptDeviceReconnect(device.name, device.deviceType);
      if (connected) {
        activeDeviceRef.current = null;
        toast.success(`${labelForDeviceType(device.deviceType)} reconnected`, {
          id: HOT_SWAP_TOAST_ID,
          description: device.name,
          duration: 5000,
        });
      } else {
        showDisconnectedToast(device);
      }
    } catch (error) {
      toast.error(`Could not reconnect ${labelForDeviceType(device.deviceType).toLowerCase()}`, {
        id: HOT_SWAP_TOAST_ID,
        description: String(error),
        action: {
          label: 'Retry',
          onClick: () => {
            retryReconnectRef.current(device);
          },
        },
        duration: Infinity,
      });
    } finally {
      reconnectingRef.current = false;
    }
  }, [showDisconnectedToast]);

  useEffect(() => {
    retryReconnectRef.current = (device) => {
      void retryReconnect(device);
    };
  }, [retryReconnect]);

  useEffect(() => {
    if (!isRecording) {
      activeDeviceRef.current = null;
      reconnectingRef.current = false;
      toast.dismiss(HOT_SWAP_TOAST_ID);
      return;
    }

    let cancelled = false;

    const poll = async () => {
      if (cancelled) return;

      try {
        const event = await recordingService.pollAudioDeviceEvents();

        if (event?.type === 'DeviceDisconnected') {
          showDisconnectedToast({
            name: event.device_name,
            deviceType: event.device_type,
          });
          return;
        }

        if (event?.type === 'DeviceReconnected') {
          const device = {
            name: event.device_name,
            deviceType: event.device_type,
          };
          activeDeviceRef.current = device;
          await retryReconnect(device);
          return;
        }

        const status = await recordingService.getReconnectionStatus();
        const disconnectedDevice = deviceFromStatus(status);
        if (disconnectedDevice && !activeDeviceRef.current) {
          showDisconnectedToast(disconnectedDevice);
        }
      } catch (error) {
        console.warn('[AudioDeviceHotSwapMonitor] Poll failed:', error);
      }
    };

    void poll();
    const intervalId = window.setInterval(() => {
      void poll();
    }, 1500);

    return () => {
      cancelled = true;
      window.clearInterval(intervalId);
    };
  }, [isRecording, retryReconnect, showDisconnectedToast]);

  return null;
}
