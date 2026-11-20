/**
 * The text of a failure, whatever it is. A command that rejects with a string
 * (the core's message, which says why) is shown as it is; so is the message of
 * an Error or of an error object that came from another context. Only when
 * there is no usable text does the fallback appear, never "Unknown error" for
 * a core message that was a plain string.
 */
const MAX_LENGTH = 400;

export function errorText(error: unknown, fallback = 'Something went wrong. Try again.'): string {
  let text = '';
  if (typeof error === 'string') text = error;
  else if (error && typeof error === 'object' && typeof (error as { message?: unknown }).message === 'string') {
    text = (error as { message: string }).message;
  }
  text = text.trim();
  return text && text.length <= MAX_LENGTH ? text : fallback;
}
