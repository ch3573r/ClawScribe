/**
 * A failed Stop leaves the recording running. The core's message says why (for
 * example that the audio device is not responding), so it is shown as it is
 * when it is short enough to be a message; anything else gets a plain one.
 */

export const STOP_FAILED_TITLE = 'Could not stop the recording';
export const STOP_FAILED_FALLBACK = 'The recording is still running. Try Stop again in a moment.';

const MAX_MESSAGE_LENGTH = 300;

function rawText(error: unknown): string {
  if (typeof error === 'string') return error;
  // An Error, or any object carrying a message (an error from another context).
  if (error && typeof error === 'object' && typeof (error as { message?: unknown }).message === 'string') {
    return (error as { message: string }).message;
  }
  return '';
}

/** Nothing was running, so there is nothing to report. */
export function isNothingToStop(error: unknown): boolean {
  return rawText(error).trim() === 'No recording in progress';
}

/** The text to show for a failed Stop. */
export function stopFailureMessage(error: unknown): string {
  const text = rawText(error).trim();
  return text && text.length <= MAX_MESSAGE_LENGTH ? text : STOP_FAILED_FALLBACK;
}
