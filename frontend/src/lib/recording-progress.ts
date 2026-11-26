export function drainEta(seconds: number | null | undefined): string | null {
  if (seconds == null || !Number.isFinite(seconds) || seconds <= 0) return null;
  const rounded = Math.ceil(seconds);
  const minutes = Math.floor(rounded / 60);
  const remainder = rounded % 60;
  const duration = [
    minutes ? `${minutes} min` : '',
    remainder ? `${remainder} sec` : '',
  ].filter(Boolean).join(' ');
  return `About ${duration} remaining`;
}
