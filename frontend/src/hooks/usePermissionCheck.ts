import { useTranscriptionConfig } from '@/contexts/ConfigContext';
import { audioSourceReadiness } from '@/lib/audioDevicePreferences';

// This reports enumerated devices. Device access itself is verified when the
// core opens a stream.
export function usePermissionCheck() {
  const { audioDeviceInventory, selectedDevices, devicePreferencesStatus } = useTranscriptionConfig();
  const { data, status, refresh } = audioDeviceInventory;
  const hasMicrophone = devicePreferencesStatus === 'ready' && data !== null &&
    audioSourceReadiness(data, 'Input', selectedDevices.micDevice) === 'available';
  const hasSystemAudio = devicePreferencesStatus === 'ready' && data !== null &&
    audioSourceReadiness(data, 'Output', selectedDevices.systemDevice) === 'available';
  return { hasMicrophone, hasSystemAudio, isChecking: status === 'checking', checkPermissions: refresh };
}
