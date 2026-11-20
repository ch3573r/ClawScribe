import type { RecordingOutcome } from './recording-outcome';

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
}

export type ArchiveAction = 'review' | 'summary' | 'transcribe' | 'files';

const archiveDate = new Intl.DateTimeFormat(undefined, {
  month: 'short', day: 'numeric', year: 'numeric', hour: '2-digit', minute: '2-digit',
});

export function formatArchiveDate(value?: string): string {
  const date = value ? new Date(value) : null;
  return date && Number.isFinite(date.getTime()) ? archiveDate.format(date) : 'Saved meeting';
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
  if (row.recording_source === 'recording') facts.push('Recording');
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
