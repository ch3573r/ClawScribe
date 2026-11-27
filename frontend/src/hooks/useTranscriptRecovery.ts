/**
 * useTranscriptRecovery Hook
 *
 * Orchestrates transcript recovery operations for interrupted meetings.
 * Provides functionality to detect, preview, and recover meetings from two
 * sources: the webview's IndexedDB and the core's scan of the recording folders.
 */

import { useState, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { indexedDBService, type MeetingMetadata, type StoredTranscript, type TranscriptPreview } from '@/services/indexedDBService';
import { applyPinnedSummaryLanguageToMeeting } from '@/lib/summary-language-preferences';
import {
  EMPTY_DISK_SCAN,
  folderKey,
  hasStableRecordingId,
  mergeRecoveryCandidates,
  readDiskScan,
  type DiskScan,
  type OfflineTranscriptSaveResult,
  type RecoveryCandidate,
} from '@/lib/interruptedRecordings';
import { toast } from 'sonner';
import { errorText } from '@/lib/errorText';

interface AudioRecoveryStatus {
  status: 'success' | 'partial' | 'failed' | 'none' | 'deferred';
  chunk_count: number;
  estimated_duration_seconds: number;
  audio_file_path?: string;
  message: string;
}

function formatCachedTranscripts(transcripts: StoredTranscript[]) {
  return transcripts.map((t, index) => ({
    id: t.id?.toString() || `${Date.now()}-${index}`,
    text: t.text,
    timestamp: t.timestamp,
    sequence_id: t.sequenceId ?? t.sequence_id ?? index,
    chunk_start_time: t.chunk_start_time,
    is_partial: t.is_partial ?? false,
    confidence: t.confidence,
    audio_start_time: t.audio_start_time,
    audio_end_time: t.audio_end_time,
    duration: t.duration,
    speaker: t.speaker ?? t.source,
    word_timestamps: t.word_timestamps,
  }));
}

async function applyRecoverySummaryLanguage(meetingId: string) {
  try {
    await applyPinnedSummaryLanguageToMeeting(meetingId);
  } catch {
    console.warn('Could not apply the default summary language to the recovered meeting.');
    toast.warning('Could not apply default summary language', {
      description: 'The recovered meeting was saved. You can choose a summary language from the meeting.',
    });
  }
}

/**
 * Every interrupted recording known right now: the browser's unsaved meetings
 * (with their audio checked) merged with what the core finds on disk, one entry
 * per folder. A failed disk scan leaves the browser cache untouched.
 */
export async function fetchRecoveryCandidates(
  options: { includeKept?: boolean } = {},
): Promise<{ candidates: RecoveryCandidate[]; scan: DiskScan; scanFailed: boolean; keptCount: number }> {
  let meetings: MeetingMetadata[];
  try {
    meetings = await indexedDBService.getAllMeetings();
  } catch {
    console.warn('Could not read the browser recovery data.');
    meetings = [];
  }

  // Give an in-progress save time to finish. Unresolved entries have no expiry.
  const handoffCutoff = Date.now() - 2_000;
  const eligibleMeetings = meetings.filter(meeting => !meeting.savedToSQLite && meeting.lastUpdated < handoffCutoff);

  let scan: DiskScan;
  try {
    const result = await invoke<DiskScan>('find_interrupted_recordings', {
      cacheEntries: eligibleMeetings.map(meeting => ({
        id: meeting.meetingId, folder: meeting.folderPath, startedAt: meeting.startTime, lastUpdated: meeting.lastUpdated,
      })),
    });
    if (!Array.isArray(result?.recordings)) throw new Error('Missing native recovery results');
    scan = readDiskScan(result);
  } catch {
    console.warn('Could not look for interrupted recordings on disk.');
    return { candidates: [], scan: EMPTY_DISK_SCAN, scanFailed: true, keptCount: 0 };
  }
  const resolvedCacheIds = new Set(scan.resolvedCacheIds);
  const detachedCacheIds = new Set(scan.detachedCacheIds);
  const unverifiedCacheIds = new Set(scan.unverifiedCacheIds);
  const unverifiedFolders = new Set<string>();
  const pendingMeetings = eligibleMeetings.filter(meeting =>
    (!scan.recordingActive || !!scan.activeCacheId) &&
    meeting.meetingId !== scan.activeCacheId && !resolvedCacheIds.has(meeting.meetingId));

  const unverified = (meeting: MeetingMetadata) => {
    unverifiedFolders.add(folderKey(meeting.folderPath));
    return { ...meeting, folderUnverified: true, recordingFolder: meeting.folderPath, folderPath: undefined };
  };

  // Verify audio checkpoint availability for each pending meeting.
  const meetingsWithAudioStatus = await Promise.all(
    pendingMeetings.map(async (meeting) => {
      if (unverifiedCacheIds.has(meeting.meetingId)) return unverified(meeting);
      if (detachedCacheIds.has(meeting.meetingId)) return { ...meeting, folderPath: undefined };
      if (meeting.folderPath) {
        try {
          const hasAudio = await invoke<boolean>('has_audio_checkpoints', {
            meetingFolder: meeting.folderPath
          });

          // If no audio files, clear folderPath to show "No audio" in UI.
          // The folder stays known for matching it with the disk scan.
          return {
            ...meeting,
            recordingFolder: meeting.folderPath,
            folderPath: hasAudio ? meeting.folderPath : undefined
          };
        } catch {
          return unverified(meeting);
        }
      }
      return meeting;
    })
  );

  const merged = mergeRecoveryCandidates(
    meetingsWithAudioStatus,
    scan.recordings.filter(recording => !unverifiedFolders.has(folderKey(recording.folder))),
  );
  // A recording the user asked not to be asked about again is left out, and
  // counted, so Settings can still offer to show it.
  const keptCount = merged.filter(candidate => candidate.disk?.kept).length;
  const candidates = options.includeKept ? merged : merged.filter(candidate => !candidate.disk?.kept);
  return { candidates, scan, scanFailed: false, keptCount };
}

interface NativeRecoveryResult {
  audioRecoveryStatus: AudioRecoveryStatus;
  meetingId: string;
  transcriptCount: number;
}

interface RecoveredMeeting {
  success: boolean;
  audioRecoveryStatus?: AudioRecoveryStatus | null;
  meetingId?: string;
  transcriptCount?: number;
  remainingCount: number;
}

export interface UseTranscriptRecoveryReturn {
  recoverableMeetings: RecoveryCandidate[];
  isLoading: boolean;
  isRecovering: boolean;
  /** The recordings found on disk could be incomplete: the scan stopped at a bound. */
  diskScanTruncated: boolean;
  scanFailed: boolean;
  hasUnverifiedFolders: boolean;
  checkForRecoverableTranscripts: (options?: { includeKept?: boolean }) => Promise<RecoveryCandidate[]>;
  /** "Keep, do not ask again": the recording stays on disk and is not offered unasked. */
  keepRecoverableMeeting: (meetingId: string) => Promise<void>;
  recoverMeeting: (meetingId: string) => Promise<RecoveredMeeting>;
  loadMeetingTranscripts: (meetingId: string) => Promise<TranscriptPreview[]>;
  saveOfflineTranscript: (meetingId: string, fileCopy?: boolean) => Promise<OfflineTranscriptSaveResult>;
  deleteRecoverableMeeting: (meetingId: string) => Promise<void>;
}

export function useTranscriptRecovery(): UseTranscriptRecoveryReturn {
  const [recoverableMeetings, setRecoverableMeetings] = useState<RecoveryCandidate[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [isRecovering, setIsRecovering] = useState(false);
  const [diskScanTruncated, setDiskScanTruncated] = useState(false);
  const [scanFailed, setScanFailed] = useState(false);
  const candidatesRef = useRef<RecoveryCandidate[]>([]);
  const scanRequest = useRef(0);
  const includeKept = useRef(false);

  const currentCandidate = useCallback((id: string) => {
    const candidate = candidatesRef.current.find(entry => entry.meetingId === id);
    if (!candidate) throw new Error('Check for interrupted recordings again before using this entry.');
    return candidate;
  }, []);

  const setCandidates = useCallback((next: RecoveryCandidate[] | ((prev: RecoveryCandidate[]) => RecoveryCandidate[])) => {
    candidatesRef.current = typeof next === 'function' ? next(candidatesRef.current) : next;
    setRecoverableMeetings(candidatesRef.current);
  }, []);

  /**
   * Check for recoverable meetings: unsaved meetings in IndexedDB and
   * interrupted recordings found in the recording folders.
   */
  const checkForRecoverableTranscripts = useCallback(async (options: { includeKept?: boolean } = {}): Promise<RecoveryCandidate[]> => {
    if (options.includeKept !== undefined) includeKept.current = options.includeKept;
    const request = ++scanRequest.current;
    setCandidates([]);
    setIsLoading(true);
    try {
      const { candidates, scan, scanFailed } = await fetchRecoveryCandidates({ includeKept: includeKept.current });
      if (request !== scanRequest.current) return [];
      setCandidates(candidates);
      setDiskScanTruncated(scan.truncated);
      setScanFailed(scanFailed);
      return candidates;
    } catch {
      console.warn('Failed to check for recoverable transcripts.');
      if (request === scanRequest.current) setScanFailed(true);
      return [];
    } finally {
      if (request === scanRequest.current) setIsLoading(false);
    }
  }, [setCandidates]);

  /**
   * Load transcripts for preview
   */
  const loadMeetingTranscripts = useCallback(async (meetingId: string): Promise<TranscriptPreview[]> => {
    try {
      const candidate = currentCandidate(meetingId);
      if (candidate?.source === 'disk' && candidate.disk) {
        // Only on disk: the core reads the first lines of the transcript snapshot.
        const lines = await invoke<Array<{ text: string; timestamp: string; audioStartTime: number }>>(
          'preview_interrupted_recording',
          { folder: candidate.disk.folder, limit: 10 },
        );
        return lines.map(line => ({
          text: line.text,
          timestamp: line.timestamp,
          audio_start_time: line.audioStartTime,
        }));
      }
      return await indexedDBService.getTranscripts(meetingId);
    } catch {
      throw new Error('Could not read the transcript preview.');
    }
  }, [currentCandidate]);

  const saveOfflineTranscript = useCallback(async (meetingId: string, fileCopy = false): Promise<OfflineTranscriptSaveResult> => {
    const candidate = currentCandidate(meetingId);
    if (!candidate.folderUnverified) throw new Error('Check the recording again before saving the transcript.');
    let transcripts;
    try {
      transcripts = formatCachedTranscripts(await indexedDBService.getTranscripts(meetingId));
    } catch {
      throw new Error('Could not read the cached transcript. Try again.');
    }
    if (!transcripts.some(row => row.text.trim())) throw new Error('There is no cached transcript to save.');
    if (currentCandidate(meetingId) !== candidate) throw new Error('The recording list changed. Review the draft again.');

    try {
      if (!fileCopy && hasStableRecordingId(meetingId)) {
        await invoke('save_offline_recording_transcript', {
          meetingId, meetingTitle: candidate.title, startedAt: candidate.startTime, transcripts,
        });
        await applyRecoverySummaryLanguage(meetingId);
        setCandidates(previous => previous.filter(entry => entry !== candidate));
        return 'library';
      }
      const saved = await invoke<boolean>('save_recovery_transcript_copy', { transcripts });
      return saved ? 'file' : 'cancelled';
    } catch {
      throw new Error('Could not save the transcript. Your draft is still available. Try again.');
    }
  }, [currentCandidate, setCandidates]);

  const recoverMeeting = useCallback(async (meetingId: string): Promise<RecoveredMeeting> => {
    setIsRecovering(true);
    try {
      const candidate = currentCandidate(meetingId);
      if (candidate.folderUnverified) {
        throw new Error('Retry the scan to verify the recordings folder before recovering the meeting.');
      }
      const fromDisk = candidate.source === 'disk' || !!candidate.disk?.registeredMeetingId;
      const folder = fromDisk ? candidate.disk?.folder : candidate.recordingFolder ?? candidate.folderPath;
      const metadata = fromDisk ? null : await indexedDBService.getMeetingMetadata(meetingId);
      if (!fromDisk && !metadata) throw new Error('Meeting metadata not found');
      const cached = metadata ? {
        id: meetingId,
        startedAt: metadata.startTime,
        lastUpdated: metadata.lastUpdated,
        hadAudio: Boolean(candidate.folderPath || candidate.disk?.hasAudio),
        transcripts: formatCachedTranscripts(await indexedDBService.getTranscripts(meetingId)),
      } : null;
      if (currentCandidate(meetingId) !== candidate) throw new Error('The recording list changed. Review the draft again.');
      const recovered = await invoke<NativeRecoveryResult>('recover_interrupted_recording', {
        folder: folder ?? null,
        meetingTitle: metadata?.title ?? candidate.title,
        cached,
      });
      if (recovered.audioRecoveryStatus.status === 'deferred') {
        toast.info('Audio repair deferred', {
          description: 'Try again when the current operation finishes. The saved meeting and recording files are kept.',
        });
        return { ...recovered, success: false, remainingCount: candidatesRef.current.length };
      }
      if (!candidate.disk?.registeredMeetingId) await applyRecoverySummaryLanguage(recovered.meetingId);
      if (cached) await indexedDBService.markMeetingSaved(meetingId);
      setCandidates(previous => previous.filter(entry => entry.meetingId !== meetingId));
      return { ...recovered, success: true, remainingCount: candidatesRef.current.length };
    } catch (error) {
      throw new Error(errorText(error, 'Could not recover the recording. Try again.'));
    } finally {
      setIsRecovering(false);
    }
  }, [currentCandidate, setCandidates]);

  /**
   * Delete a recoverable meeting. A recording that exists only on disk is
   * deleted by the core (validated, only an unsaved interrupted folder), and
   * only because the user asked; nothing is ever deleted automatically.
   */
  const deleteRecoverableMeeting = useCallback(async (meetingId: string): Promise<void> => {
    try {
      const candidate = currentCandidate(meetingId);
      if (candidate.folderUnverified) {
        throw new Error('Retry the scan to verify the recordings folder before deleting this draft.');
      }
      if (candidate.disk?.registeredMeetingId) {
        throw new Error('This recording is saved in your library and cannot be deleted from here.');
      }
      if (candidate.source === 'disk' && candidate.disk) {
        await invoke('discard_interrupted_recording', { folder: candidate.disk.folder });
      } else {
        await indexedDBService.deleteMeeting(meetingId);
      }
      setCandidates(prev => prev.filter(m => m.meetingId !== meetingId));
    } catch (error) {
      console.error('Failed to delete meeting:', error);
      throw error;
    }
  }, [currentCandidate, setCandidates]);

  const keepRecoverableMeeting = useCallback(async (meetingId: string): Promise<void> => {
    const candidate = currentCandidate(meetingId);
    const folder = candidate?.disk?.folder;
    if (!folder) throw new Error('This recording is not on disk and cannot be kept this way.');
    await invoke('keep_interrupted_recording', { folder, keep: true });
    setCandidates(prev => prev.filter(m => m.meetingId !== meetingId));
  }, [currentCandidate, setCandidates]);

  return {
    recoverableMeetings,
    isLoading,
    isRecovering,
    diskScanTruncated,
    scanFailed,
    hasUnverifiedFolders: recoverableMeetings.some(entry => entry.folderUnverified),
    checkForRecoverableTranscripts,
    keepRecoverableMeeting,
    recoverMeeting,
    loadMeetingTranscripts,
    saveOfflineTranscript,
    deleteRecoverableMeeting
  };
}
