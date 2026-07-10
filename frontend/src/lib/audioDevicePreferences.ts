export interface SelectedDevices {
  micDevice: string | null;
  systemDevice: string | null;
}

export interface AudioDevicePreferenceFields {
  preferred_mic_device: string | null;
  preferred_system_device: string | null;
}

export function selectedDevicesFromPreferences(
  preferences: AudioDevicePreferenceFields,
): SelectedDevices {
  return {
    micDevice: preferences.preferred_mic_device,
    systemDevice: preferences.preferred_system_device,
  };
}

export function getAudioDeviceDisplayName(
  deviceName: string | null,
  fallback: string,
): string {
  if (!deviceName) return fallback;

  return deviceName.replace(/\s+\((input|output)\)$/i, '').trim() || fallback;
}

export function isExplicitAudioDevice(deviceName: string | null): boolean {
  return Boolean(deviceName?.trim());
}
