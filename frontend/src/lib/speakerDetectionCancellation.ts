import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';

export async function offerSpeakerDetectionCancellation(error: unknown, retry: () => Promise<unknown>): Promise<void> {
  if (!String(error).toLowerCase().includes('speaker detection')) return;
  const meetingId = await invoke<string | null>('active_speaker_diarization_command');
  if (!meetingId) return;
  let pending = false;
  toast.error('Speaker detection is running', {
    duration: 15000,
    action: {
      label: 'Cancel speaker detection and record',
      onClick: async () => {
        if (pending) return;
        pending = true;
        const notice = toast.loading('Cancelling speaker detection after the current speech step…');
        try {
          await invoke('cancel_speaker_diarization_command', { meetingId });
          const deadline = Date.now() + 60000;
          while (await invoke<string | null>('active_speaker_diarization_command') === meetingId) {
            if (Date.now() >= deadline) throw new Error('Speaker detection is still finishing its current step. Try recording again shortly.');
            await new Promise(resolve => setTimeout(resolve, 250));
          }
          await retry();
        } catch (failure) {
          toast.error('Could not start recording', { description: String(failure) });
        } finally {
          toast.dismiss(notice);
          pending = false;
        }
      },
    },
  });
}
