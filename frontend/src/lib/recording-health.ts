import type { CaptureDetails } from '@/lib/recording-outcome';

export interface MissingRecordingDevice {
  name: string;
  device_type: 'Microphone' | 'SystemAudio';
}

export interface RecordingHealth {
  warnings: string[];
  disconnected_devices: MissingRecordingDevice[];
  capture_incomplete: boolean;
  reconnecting: boolean;
  destination_free_bytes: number | null;
  scratch_free_bytes: number | null;
  details: CaptureDetails;
  transcription: {
    chunks_in_queue: number;
    queued_audio_seconds: number;
    estimated_seconds_remaining: number | null;
    failed_chunks: number;
  } | null;
}

export const deviceLabel = (device: MissingRecordingDevice) =>
  device.device_type === 'Microphone' ? 'Microphone' : 'System audio';

export function recordingHealthWarnings(health: RecordingHealth) {
  const warnings = [...health.warnings];
  if (health.capture_incomplete && health.details.dropped_samples === 0) {
    warnings.push('The saved meeting may have a capture gap.');
  }
  if (health.transcription?.failed_chunks) {
    warnings.push('Live transcription is incomplete. Check the saved audio and use Retranscribe to recover missing speech.');
  }
  if (health.details.backend?.includes('CPU fallback')) {
    warnings.push('Live transcription is using CPU fallback. Check acceleration settings if transcription falls behind.');
  }
  if (health.scratch_free_bytes != null && health.scratch_free_bytes < 250_000_000) {
    warnings.push('Transcription scratch space is low. Free disk space or stop and save the recording.');
  }
  if (health.destination_free_bytes != null && health.destination_free_bytes < 777_600_000) {
    warnings.push('Recording destination space is low. Free disk space or stop and save the recording.');
  }
  return warnings;
}

export function recordingHealthMetrics(health: RecordingHealth) {
  const items: string[] = [];
  if (health.details.backend) items.push(health.details.backend);
  if (health.transcription?.chunks_in_queue) {
    items.push(`${Math.ceil(health.transcription.queued_audio_seconds)} s of speech waiting or processing`);
  }
  if (health.destination_free_bytes != null) {
    items.push(`${(health.destination_free_bytes / 1e9).toFixed(1)} GB free for recording`);
  }
  if (health.scratch_free_bytes != null) {
    items.push(`${(health.scratch_free_bytes / 1e9).toFixed(1)} GB free for transcription scratch`);
  }
  return items;
}
