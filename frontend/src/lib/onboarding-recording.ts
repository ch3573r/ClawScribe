import { invoke } from '@tauri-apps/api/core';
import type { RecordingMode } from '@/contexts/RecordingStateContext';

export async function finishRecordingSetup(
  mode: RecordingMode,
  localNotesModel: string | null,
) {
  const firstLaunch = await invoke<boolean>('check_first_launch').catch(() => {
    throw new Error('Could not check the meeting library. Reopen ClawScribe and retry setup.');
  });
  if (firstLaunch) {
    const imported = await invoke<boolean>('import_legacy_database_if_present').catch(() => {
      throw new Error('Could not import your existing meeting library. Check that its database is accessible and try again.');
    });
    if (!imported) await invoke('initialize_fresh_database').catch(() => {
      throw new Error('Could not open the meeting library. Check available disk space and write access, then retry setup.');
    });
  }
  await invoke('complete_onboarding', { mode, localNotesModel }).catch((cause: unknown) => {
    if (String(cause).includes('No speech model is available')) {
      throw new Error('The speech model is unavailable. Set up live transcription again or choose Record audio now.');
    }
    throw new Error('Could not save your recording setup. Try again.');
  });
}
