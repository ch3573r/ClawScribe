import { useRouter } from 'next/navigation';
import { AudioLines } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { useBackgroundImportQueue } from '@/contexts/ImportDialogContext';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { errorText } from '@/lib/errorText';
import type { ImportQueueItem } from '@/hooks/useImportAudio';

export function ImportSpeakerDetectionButton({ result, compact }: {
  result: NonNullable<ImportQueueItem['result']>;
  compact?: boolean;
}) {
  const router = useRouter();
  const { detecting, detected, detectSpeakers } = useBackgroundImportQueue();
  const { isRecording, isStarting, isStopping, isProcessing, isSaving } = useRecordingState();
  const recordingBusy = isRecording || isStarting || isStopping || isProcessing || isSaving;
  const id = result.meeting_id;
  if (result.segments_count <= 0 || result.files_pending || detected.has(id)) return null;
  const label = detecting === id ? 'Detecting speakers…' : 'Detect speakers';
  const detect = async () => {
    if (detecting || recordingBusy) return;
    try {
      if (await detectSpeakers(id)) {
        toast.success('Speaker detection finished', {
          action: { label: 'Open meeting', onClick: () => router.push(`/meeting-details?id=${encodeURIComponent(id)}`) },
        });
      }
    } catch (error) {
      toast.error('Could not detect speakers', { description: errorText(error) });
    }
  };
  return <Button size={compact ? 'icon' : 'sm'} variant={compact ? 'ghost' : 'outline'}
    disabled={Boolean(detecting) || recordingBusy} aria-label={label}
    title={recordingBusy ? 'Available after the recording is saved' : compact ? label : undefined}
    onClick={() => void detect()}>
    {compact ? <AudioLines aria-hidden="true" /> : label}
  </Button>;
}
