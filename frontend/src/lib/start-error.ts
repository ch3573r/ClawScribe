/** The text of an error from a Tauri command, which rejects with a plain string. */
export function startErrorText(error: unknown): string {
  if (typeof error === "string") return error;
  const message = (error as { message?: unknown } | null | undefined)?.message;
  if (typeof message === "string") return message;
  return String(error);
}

/**
 * The core refuses to start a recording on a nearly full volume with two
 * sentences: how much is free where recordings are saved, and what to do. That
 * message is already the explanation; it must reach the user whole, and must not
 * be mistaken for a microphone or model problem because its folder path happens
 * to contain a word such as "mic" or "model".
 */
export function isDiskSpaceRefusal(error: unknown): boolean {
  return startErrorText(error).startsWith("Not enough free disk space to record.");
}

export interface StartErrorDisplay { title: string; message: string }

/**
 * What the Start button's error panel says. A refused start for a full disk
 * carries its own complete explanation and is shown whole, ahead of the guesses
 * below, which look for words such as "mic" and "output" anywhere in the text
 * (a folder path can contain them).
 */
export function describeStartError(error: unknown): StartErrorDisplay {
  if (isDiskSpaceRefusal(error)) return { title: 'Not enough disk space', message: startErrorText(error) };
  const text = startErrorText(error);
  if (text.includes('microphone') || text.includes('mic') || text.includes('input')) {
    return {
      title: 'Microphone not available',
      message: 'Unable to access your microphone. Please check that:\n• Your microphone is connected\n• The app has microphone permissions\n• No other app is using the microphone',
    };
  }
  if (text.includes('system audio') || text.includes('speaker') || text.includes('output')) {
    return {
      title: 'System audio not available',
      message: 'Unable to capture system audio. Please check that:\n• An audio output device is connected and enabled\n• Another app is not using it exclusively\n• Your sound settings are correct',
    };
  }
  if (text.includes('permission')) {
    return {
      title: 'Permission required',
      message: 'Recording permissions are required. Please:\n• Allow microphone access for ClawScribe in your system privacy settings\n• Restart the app after changing permissions',
    };
  }
  return { title: 'Recording failed', message: 'Unable to start recording. Please check your audio device settings and try again.' };
}
