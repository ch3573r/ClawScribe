export interface SelectedDevices {
  micDevice: string | null;
  systemDevice: string | null;
}

export interface AudioDevice {
  name: string;
  device_type: 'Input' | 'Output';
}

export interface AudioDeviceInventory {
  devices: AudioDevice[];
  defaultMicrophone: string | null;
  defaultOutput: string | null;
}

export function audioSourceReadiness(
  inventory: AudioDeviceInventory,
  type: AudioDevice['device_type'],
  preferred: string | null,
): 'available' | 'preferred_unavailable' | 'default_unavailable' {
  if (preferred) {
    return inventory.devices.some(device =>
      device.device_type === type && `${device.name} (${type.toLowerCase()})` === preferred,
    ) ? 'available' : 'preferred_unavailable';
  }
  const defaultDevice = type === 'Input' ? inventory.defaultMicrophone : inventory.defaultOutput;
  return defaultDevice ? 'available' : 'default_unavailable';
}

export function canEditLiveAudioSaving(mode: 'live' | 'audio_only' | null): boolean {
  return mode === 'live';
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
