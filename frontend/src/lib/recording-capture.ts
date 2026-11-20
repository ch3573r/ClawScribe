import type { RecordingState } from '@/services/recordingService';

/**
 * The core keeps a session open after a fatal capture error: it stops taking
 * audio (the session reports neither paused nor active) but waits for Stop to
 * save what was captured. A paused session is also not active, so the pause
 * flag tells the two apart.
 */
export function captureHalted(state: Pick<RecordingState, 'is_recording' | 'is_paused' | 'is_active'>): boolean {
  return state.is_recording && !state.is_paused && !state.is_active;
}

export function recordingErrorMessage(payload: unknown): string {
  const text = typeof payload === 'string' ? payload.trim() : '';
  return text || 'Audio capture failed';
}
