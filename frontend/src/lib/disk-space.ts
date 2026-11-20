/** Payload of the core's `recording-disk-space-low`, sent once per recording. */
export interface DiskSpaceLowPayload {
  freeBytes: number;
  folder?: string;
}

/**
 * Free space in a readable unit. Decimal units, like the core's own messages
 * ("1.6 GB is needed"): a gigabyte with one decimal from 1 GB up, whole
 * megabytes below that. Returns null when the number is not usable.
 */
export function formatFreeSpace(bytes: unknown): string | null {
  if (typeof bytes !== "number" || !Number.isFinite(bytes) || bytes < 0) return null;
  if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
  return `${Math.round(bytes / 1_000_000)} MB`;
}

/**
 * The warning's text. It says how much is free, that the recording goes on, and
 * what can happen. The folder is not part of it.
 */
export function diskSpaceLowText(payload: Partial<DiskSpaceLowPayload> | null | undefined): string {
  const free = formatFreeSpace(payload?.freeBytes);
  return `${free ? `${free} is free where recordings are saved. ` : 'Free space is low where recordings are saved. '}The recording continues, but it may be cut short if the disk fills up.`;
}
