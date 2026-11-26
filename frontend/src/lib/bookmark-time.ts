// The core accepts recording offsets up to seven days.
const MAX_BOOKMARK_SECONDS = 7 * 24 * 60 * 60;

export function parseBookmarkTime(input: string, duration?: number): number | null {
  const time = input.trim();
  const clock = /^(?:(\d+):)?(\d+):([0-5]\d)$/.exec(time);
  let seconds: number;
  if (clock) {
    const [, hours, minutes, remainder] = clock;
    if (hours !== undefined && !/^[0-5]\d$/.test(minutes)) return null;
    seconds = Number(hours ?? 0) * 3600 + Number(minutes) * 60 + Number(remainder);
  } else if (/^\d+(?:\.\d+)?$/.test(time)) {
    seconds = Number(time);
  } else {
    return null;
  }

  if (!Number.isFinite(seconds)) return null;
  if (duration !== undefined && Number.isFinite(duration) && duration > 0) seconds = Math.min(seconds, duration);
  return seconds <= MAX_BOOKMARK_SECONDS ? seconds : null;
}
