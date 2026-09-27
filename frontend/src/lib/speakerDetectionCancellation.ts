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
        const notice = toast.loading('Stopping speaker detection. On long meetings the current step can take a few minutes.');
        try {
          await invoke('cancel_speaker_diarization_command', { meetingId });
          const deadline = Date.now() + 5 * 60 * 1000;
          while (await invoke<string | null>('active_speaker_diarization_command') === meetingId) {
            if (Date.now() >= deadline) throw new Error('Speaker detection is still finishing its current step. Recording did not start. Try again in a moment.');
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
