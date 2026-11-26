import type { RecordingOutcome } from './recording-outcome';

interface ArchiveSeries {
  id: string;
  label: string;
  date?: string;
}

export interface ArchiveRow extends RecordingOutcome {
  id: string;
  has_folder: boolean;
  has_transcript: boolean;
  has_summary: boolean;
  summary_status: string | null;
  pending_file_sync: boolean;
  /** Native capture or file import, when known. */
  recording_source: 'recording' | 'import' | null;
  /** Measured duration in seconds, when available. */
  duration_seconds: number | null;
  series?: ArchiveSeries | null;
}

export function archiveSeriesChoices(rows: Iterable<ArchiveRow>): ArchiveSeries[] {
  const choices = new Map<string, ArchiveSeries>();
  for (const { series } of rows) {
    if (series && !choices.has(series.id)) choices.set(series.id, series);
  }
  return [...choices.values()].sort((a, b) => a.label.localeCompare(b.label));
}

export type ArchiveAction = 'review' | 'summary' | 'transcribe' | 'files';

const archiveDate = new Intl.DateTimeFormat(undefined, {
  month: 'short', day: 'numeric', year: 'numeric', hour: '2-digit', minute: '2-digit',
});
const archiveDay = new Intl.DateTimeFormat(undefined, {
  month: 'short', day: 'numeric', year: 'numeric',
});
const calendarDate = /^\d{4}-\d{2}-\d{2}$/;

export function parseArchiveDate(value?: string): Date | null {
  if (!value) return null;
  const hasDayOnly = calendarDate.test(value);
  const iso = value.replace(/^(\d{4}-\d{2}-\d{2}) /, '$1T');
  // SQLite's legacy timestamps omit the zone and were recorded in UTC.
  const date = new Date(hasDayOnly ? `${iso}T12:00:00`
    : /(?:Z|[+-]\d{2}:?\d{2})$/i.test(iso) ? iso : `${iso}Z`);
  if (!Number.isFinite(date.getTime())) return null;
  if (hasDayOnly && localDateKey(date) !== value) return null;
  return date;
}

function localDateKey(date: Date): string {
  const year = String(date.getFullYear()).padStart(4, '0');
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

export function archiveDateKey(value?: string): string | null {
  const date = parseArchiveDate(value);
  return date ? localDateKey(date) : null;
}

export function formatArchiveDate(value?: string): string {
  const date = parseArchiveDate(value);
  if (!date) return 'Saved meeting';
  return (calendarDate.test(value ?? '') ? archiveDay : archiveDate).format(date);
}

export function archiveReview(row: ArchiveRow): {
  needsAttention: boolean;
  action: ArchiveAction;
  transcript: string;
  summary: string;
  warnings: string[];
} {
  const summaryFailed = ['failed', 'error', 'cancelled'].includes(row.summary_status ?? '');
  const summaryRunning = ['pending', 'processing', 'summarizing', 'regenerating'].includes(row.summary_status ?? '');
  const summary = summaryFailed ? 'Summary needs retry' : summaryRunning ? 'Summary running' : row.has_summary ? 'Summary saved' : 'No summary';
  const transcript = row.transcription_incomplete ? 'Transcript incomplete' : row.has_transcript ? 'Transcript saved' : 'No transcript';
  const warnings: string[] = [];
  const localRecovery = row.audio_save_failed || row.transcription_incomplete;
  if (row.recovery_files_elsewhere) warnings.push('Recovery files are on the original computer');
  if (row.audio_save_failed) warnings.push('Audio needs recovery');
  if (row.capture_incomplete) warnings.push('Audio was lost during capture');
  if (row.pending_file_sync) warnings.push('Recording-folder copy needs updating');
  else if (row.recording_files_incomplete) warnings.push('Recording files need review');
  const base = { transcript, summary, warnings };

  if (row.recovery_files_elsewhere) {
    return { ...base, needsAttention: true, action: 'review' };
  }
  if (localRecovery && row.has_folder) {
    return { ...base, needsAttention: true, action: 'transcribe' };
  }
  if (row.pending_file_sync || row.recording_files_incomplete) {
    return { ...base, needsAttention: true, action: row.has_folder && row.has_transcript ? 'files' : 'review' };
  }
  if (row.capture_incomplete || localRecovery) {
    return { ...base, needsAttention: true, action: 'review' };
  }
  if (row.has_folder && !row.has_transcript) {
    return { ...base, needsAttention: true, action: 'transcribe' };
  }
  return {
    ...base,
    needsAttention: summaryFailed,
    action: row.has_transcript && !summaryRunning && (summaryFailed || !row.has_summary) ? 'summary' : 'review',
  };
}

export function visibleSelection(selected: ReadonlySet<string>, visible: readonly string[]): Set<string> {
  return new Set(visible.filter(id => selected.has(id)));
}

export function archiveRecordingFacts(row: Pick<ArchiveRow, 'recording_source' | 'duration_seconds'>): string[] {
  const facts: string[] = [];
  if (row.recording_source === 'recording') facts.push('Recorded');
  if (row.recording_source === 'import') facts.push('Imported audio');
  const duration = row.duration_seconds;
  if (duration != null && Number.isFinite(duration) && duration > 0) {
    const seconds = Math.round(duration);
    const hours = Math.floor(seconds / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    const remainingSeconds = seconds % 60;
    facts.push(hours > 0 ? `${hours}h ${minutes}m`
      : minutes > 0 ? `${minutes}m ${remainingSeconds}s`
      : `${remainingSeconds}s`);
  }
  return facts;
}

export function formatArchiveBytes(bytes: number): string {
  const number = new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 });
  if (bytes >= 1024 ** 3) return `${number.format(bytes / 1024 ** 3)} GiB`;
  if (bytes >= 1024 ** 2) return `${number.format(bytes / 1024 ** 2)} MiB`;
  if (bytes >= 1024) return `${number.format(bytes / 1024)} KiB`;
  return `${number.format(bytes)} B`;
}
