export interface RecordingOutcome {
  audio_save_failed: boolean;
  transcription_incomplete: boolean;
}

export function recordingOutcome(value?: Partial<RecordingOutcome> | null): RecordingOutcome {
  return {
    audio_save_failed: value?.audio_save_failed === true,
    transcription_incomplete: value?.transcription_incomplete === true,
  };
}

export function recordingRecoveryMessage(value: RecordingOutcome): string | null {
  if (value.audio_save_failed) {
    return 'Audio could not be fully saved. Check disk space and use Retranscribe to recover the available audio. Keep the meeting recovery files.';
  }
  if (value.transcription_incomplete) {
    return 'The live transcript is incomplete. Use Retranscribe to process the saved audio before relying on notes or exports.';
  }
  return null;
}
