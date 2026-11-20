import { invoke } from '@tauri-apps/api/core';
import type { RecordingMode } from '@/contexts/RecordingStateContext';

export async function finishRecordingSetup(
  mode: RecordingMode,
  localNotesModel: string | null,
) {
  if (await invoke<boolean>('check_first_launch')) {
    const imported = await invoke<boolean>('import_legacy_database_if_present').catch(() => false);
    if (!imported) await invoke('initialize_fresh_database');
  }
  await invoke('complete_onboarding', { mode, localNotesModel });
}
