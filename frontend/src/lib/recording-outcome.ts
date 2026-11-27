export interface CaptureDetails {
  dropped_samples: number;
  system_silence_periods: number;
  backend: string | null;
  unfinished_segments: number;
  empty_live_transcript: 'no_speech_detected' | 'no_text_recognized' | 'incomplete' | null;
}

export interface RecordingOutcome {
  audio_save_failed: boolean;
  transcription_incomplete: boolean;
  capture_incomplete?: boolean;
  recording_files_incomplete?: boolean;
  recovery_files_elsewhere?: boolean;
  capture_details?: CaptureDetails | null;
}

export function recordingOutcome(value?: Partial<RecordingOutcome> | null): RecordingOutcome {
  return {
    audio_save_failed: value?.audio_save_failed === true,
    transcription_incomplete: value?.transcription_incomplete === true,
    capture_incomplete: value?.capture_incomplete === true,
    recording_files_incomplete: value?.recording_files_incomplete === true,
    recovery_files_elsewhere: value?.recovery_files_elsewhere === true,
    ...(value?.capture_details ? { capture_details: value.capture_details } : {}),
  };
}

export function emptyLiveTranscriptMessage(reason: CaptureDetails['empty_live_transcript']) {
  switch (reason) {
    case 'no_speech_detected':
      return 'Live transcription detected no speech. Play the recording to check what was captured.';
    case 'no_text_recognized':
      return 'Live transcription processed speech but returned no text. Check the recording and try retranscribing.';
    case 'incomplete':
      return 'The live transcript ended empty after a capture or transcription problem. Check the audio before retranscribing.';
    default:
      return null;
  }
}

export function recordingBlocksAutoSummary(value?: Partial<RecordingOutcome> | null): boolean {
  return value?.audio_save_failed === true || value?.transcription_incomplete === true;
}

export function recordingRecoveryMessage(value: RecordingOutcome, audioSaved?: boolean): string | null {
  const messages: string[] = [];
  if (value.recovery_files_elsewhere) {
    messages.push('Audio could not be fully saved on the computer where this meeting was recorded. Its recovery files were not included in this backup. Recover the audio on that computer, then back it up again.');
  } else if (value.audio_save_failed) {
    messages.push('Audio could not be fully saved. Check disk space and use Retranscribe to recover the available audio. Keep the meeting recovery files.');
  }
  if (value.capture_incomplete) {
    messages.push('Some audio was lost during capture. Notes can cover only the saved transcript. Retranscription cannot restore missing sound.');
  }
  if (value.transcription_incomplete && !value.recovery_files_elsewhere) {
    const remaining = value.capture_details?.unfinished_segments ?? 0;
    messages.push('The live transcript is incomplete.');
    if (remaining > 0) {
      messages.push(`Stop left ${remaining} speech ${remaining === 1 ? 'segment' : 'segments'} unfinished.`);
    }
    if (audioSaved) {
      messages.push(remaining > 0
        ? 'Use Retranscribe to process the saved audio.'
        : 'Use Retranscribe to process the saved audio before relying on notes or exports.');
    } else if (audioSaved === false && !value.audio_save_failed) {
      messages.push('Audio was not kept.');
    } else if (!value.audio_save_failed) {
      messages.push('If audio is available, use Retranscribe before relying on notes or exports.');
    }
  }
  if (value.recording_files_incomplete && !value.recovery_files_elsewhere) {
    messages.push('Some recording details or transcript backup files could not be saved. Review the library transcript and keep the meeting folder.');
  }
  return messages.length ? messages.join(' ') : null;
}
