import type { audioSourceReadiness } from '@/lib/audioDevicePreferences';
import type { RecordingMode } from '@/contexts/RecordingStateContext';

type SourceReadiness = ReturnType<typeof audioSourceReadiness> | null;

export function startReadinessReason({
  inventoryStatus, preferencesStatus, mic, system, mode, hasReadiness, readinessError,
  modelIssue, savingDevices, savingMode,
}: {
  inventoryStatus: string;
  preferencesStatus: string;
  mic: SourceReadiness;
  system: SourceReadiness;
  mode: RecordingMode | null;
  hasReadiness: boolean;
  readinessError: boolean;
  modelIssue: string | null | undefined;
  savingDevices: boolean;
  savingMode: boolean;
}): string | null {
  if (inventoryStatus === 'error' || preferencesStatus === 'error') return 'Could not check audio devices';
  if (inventoryStatus === 'checking' || preferencesStatus === 'loading') return 'Checking audio devices';
  if (mic === 'preferred_unavailable') return 'Preferred microphone unavailable';
  if (mic !== 'available') return 'Default microphone unavailable';
  if (system === 'preferred_unavailable') return 'Preferred output unavailable';
  if (!mode) return 'Choose a recording mode';
  if (mode === 'live') {
    if (readinessError) return 'Could not check speech model';
    if (!hasReadiness) return 'Checking speech model';
    if (modelIssue) return 'Live transcription needs attention';
  }
  if (savingDevices || savingMode) return 'Saving recording settings…';
  return null;
}

export function recordingActionBlock({
  isRecording, isStarting, isStopping, isProcessing, isSaving,
}: {
  isRecording?: boolean;
  isStarting?: boolean;
  isStopping?: boolean;
  isProcessing?: boolean;
  isSaving?: boolean;
}, startReason: string | null): string | null {
  if (isStopping || isProcessing || isSaving) return 'Saving recording…';
  if (isStarting) return 'Starting recording…';
  return isRecording ? null : startReason;
}
