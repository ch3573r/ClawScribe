import type { MeetingMetadata } from "@/services/indexedDBService";

/**
 * Interrupted recordings come from two places: the webview's IndexedDB (which
 * holds the live transcript) and the core's scan of the recording folders (which
 * finds what is on disk even when the webview storage is gone or was never
 * written). This module merges them, one entry per recording folder.
 */

/** What the core's scan reports for one folder. */
export interface DiskRecording {
  folder: string;
  title: string | null;
  startedAt: string | null;
  approxDurationSeconds: number | null;
  sizeBytes: number;
  hasTranscript: boolean;
  transcriptLines: number;
  hasCapture: boolean;
  hasAudio: boolean;
  reasons: string[];
  reason: string;
  registeredMeetingId: string | null;
  /** The user asked not to be asked about this recording again. */
  kept: boolean;
}

export interface DiskScan {
  recordings: DiskRecording[];
  truncated: boolean;
  recordingActive: boolean;
  activeCacheId?: string;
  resolvedCacheIds?: string[];
  detachedCacheIds?: string[];
  unverifiedCacheIds?: string[];
}

export type RecoverySource = "browser" | "disk";

/** An entry of the recovery list. `disk` is set when the folder is known to the scan. */
export type RecoveryCandidate = MeetingMetadata & {
  source: RecoverySource;
  recordingFolder?: string;
  disk?: DiskRecording;
};

export const EMPTY_DISK_SCAN: DiskScan = { recordings: [], truncated: false, recordingActive: false };

const DISK_PREFIX = "disk:";

/** Set by Settings to ask the home page to open the recovery dialog. */
export const RECOVERY_OPEN_REQUEST = "recovery_open_requested";
/** The value that also shows the recordings the user asked not to be asked about. */
export const RECOVERY_OPEN_ALL = "all"; 

/** A folder path for comparison: slash direction, case and a trailing separator do not matter. */
export function folderKey(path: string | null | undefined): string {
  if (!path) return "";
  const text = path.replace(/\\/g, "/").toLowerCase();
  const trimmed = text.replace(/\/+$/, "");
  return trimmed === "" || trimmed.endsWith(":") ? text : trimmed;
}

export function isDiskCandidateId(meetingId: string): boolean {
  return meetingId.startsWith(DISK_PREFIX);
}

function text(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value : null;
}

function readRecording(value: unknown): DiskRecording | null {
  if (!value || typeof value !== "object") return null;
  const source = value as Record<string, unknown>;
  const folder = text(source.folder);
  if (!folder) return null;
  return {
    folder,
    title: text(source.title),
    startedAt: text(source.startedAt),
    approxDurationSeconds: typeof source.approxDurationSeconds === "number" ? source.approxDurationSeconds : null,
    sizeBytes: typeof source.sizeBytes === "number" ? source.sizeBytes : 0,
    hasTranscript: source.hasTranscript === true,
    transcriptLines: typeof source.transcriptLines === "number" ? source.transcriptLines : 0,
    hasCapture: source.hasCapture === true,
    hasAudio: source.hasAudio === true,
    reasons: Array.isArray(source.reasons) ? source.reasons.filter((reason): reason is string => typeof reason === "string") : [],
    reason: text(source.reason) ?? "The recording was interrupted",
    registeredMeetingId: text(source.registeredMeetingId),
    kept: source.kept === true,
  };
}

/** A scan result from the core, tolerating anything unexpected. */
export function readDiskScan(value: unknown): DiskScan {
  if (!value || typeof value !== "object") return EMPTY_DISK_SCAN;
  const source = value as Record<string, unknown>;
  const recordings = Array.isArray(source.recordings)
    ? source.recordings.map(readRecording).filter((recording): recording is DiskRecording => recording !== null)
    : [];
  const resolvedCacheIds = Array.isArray(source.resolvedCacheIds)
    ? source.resolvedCacheIds.filter((id): id is string => typeof id === 'string') : [];
  const activeCacheId = text(source.activeCacheId);
  const detachedCacheIds = Array.isArray(source.detachedCacheIds)
    ? source.detachedCacheIds.filter((id): id is string => typeof id === 'string') : [];
  const unverifiedCacheIds = Array.isArray(source.unverifiedCacheIds)
    ? source.unverifiedCacheIds.filter((id): id is string => typeof id === 'string') : [];
  return { recordings, truncated: source.truncated === true, recordingActive: source.recordingActive === true,
    ...(activeCacheId ? { activeCacheId } : {}),
    ...(resolvedCacheIds.length > 0 ? { resolvedCacheIds } : {}),
    ...(detachedCacheIds.length > 0 ? { detachedCacheIds } : {}),
    ...(unverifiedCacheIds.length > 0 ? { unverifiedCacheIds } : {}) };
}

/** A recording found only on disk, shaped like an entry of the browser list. */
export function diskCandidate(recording: DiskRecording): RecoveryCandidate {
  const started = recording.startedAt ? Date.parse(recording.startedAt) : NaN;
  const startTime = Number.isFinite(started) ? started : 0;
  return {
    meetingId: DISK_PREFIX + recording.folder,
    title: recording.title ?? "Untitled recording",
    startTime,
    lastUpdated: startTime,
    transcriptCount: recording.transcriptLines,
    savedToSQLite: false,
    // The list shows "Audio available" for an entry with a folder.
    folderPath: recording.hasAudio ? recording.folder : undefined,
    source: "disk",
    disk: recording,
  };
}

/**
 * One entry per recording folder. An entry known from the browser keeps its own
 * data (it holds the transcript the webview collected) and gets the disk
 * information attached; a folder known only to the scan becomes a disk entry.
 * Entries without a folder stay as they are.
 */
export function mergeRecoveryCandidates(
  browser: Array<MeetingMetadata & { recordingFolder?: string }>,
  disk: DiskRecording[],
): RecoveryCandidate[] {
  const byFolder = new Map(disk.map(recording => [folderKey(recording.folder), recording]));
  const claimed = new Set<string>();
  const merged: RecoveryCandidate[] = browser.map(entry => {
    const { recordingFolder, ...meeting } = entry;
    const key = folderKey(recordingFolder ?? entry.folderPath);
    const recording = key ? byFolder.get(key) : undefined;
    if (recording) claimed.add(key);
    return { ...meeting, ...(recordingFolder ? { recordingFolder } : {}), source: "browser" as const, ...(recording ? { disk: recording } : {}) };
  });
  for (const recording of disk) {
    const key = folderKey(recording.folder);
    if (!claimed.has(key)) {
      claimed.add(key);
      merged.push(diskCandidate(recording));
    }
  }
  return merged.sort((a, b) => b.lastUpdated - a.lastUpdated);
}

/** "1 h 5 min", "12 min", "under a minute": an approximate length for the list. */
export function describeDuration(seconds: number | null | undefined): string | null {
  if (typeof seconds !== "number" || !Number.isFinite(seconds) || seconds <= 0) return null;
  if (seconds < 60) return "under a minute";
  const minutes = Math.round(seconds / 60);
  return minutes < 60 ? `${minutes} min` : `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

export function describeSize(bytes: number | null | undefined): string | null {
  if (typeof bytes !== "number" || !Number.isFinite(bytes) || bytes <= 0) return null;
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${Math.round(bytes / (1024 * 1024))} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}
