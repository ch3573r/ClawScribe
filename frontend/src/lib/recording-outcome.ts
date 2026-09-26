export interface RecordingOutcome {
  audio_save_failed: boolean;
  transcription_incomplete: boolean;
  capture_incomplete?: boolean;
  recording_files_incomplete?: boolean;
}

export function recordingOutcome(value?: Partial<RecordingOutcome> | null): RecordingOutcome {
  return {
    audio_save_failed: value?.audio_save_failed === true,
    transcription_incomplete: value?.transcription_incomplete === true,
    capture_incomplete: value?.capture_incomplete === true,
    recording_files_incomplete: value?.recording_files_incomplete === true,
  };
}

export function recordingRecoveryMessage(value: RecordingOutcome): string | null {
  const messages: string[] = [];
  if (value.audio_save_failed) {
    messages.push('Audio could not be fully saved. Check disk space and use Retranscribe to recover the available audio. Keep the meeting recovery files.');
  }
  if (value.capture_incomplete) {
    messages.push('Some audio was lost during capture. Review the saved audio and transcript; retranscription cannot restore missing sound.');
  }
  if (value.transcription_incomplete) {
    messages.push('The live transcript is incomplete. Use Retranscribe to process the saved audio before relying on notes or exports.');
  }
  if (value.recording_files_incomplete) {
    messages.push('Some recording details or transcript backup files could not be saved. Review the library transcript and keep the meeting folder.');
  }
  return messages.length ? messages.join(' ') : null;
}
