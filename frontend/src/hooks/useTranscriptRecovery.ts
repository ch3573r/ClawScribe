/**
 * useTranscriptRecovery Hook
 *
 * Orchestrates transcript recovery operations for interrupted meetings.
 * Provides functionality to detect, preview, and recover meetings from two
 * sources: the webview's IndexedDB and the core's scan of the recording folders.
 */

import { useState, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { indexedDBService, type MeetingMetadata, type TranscriptPreview } from '@/services/indexedDBService';
import { storageService } from '@/services/storageService';
import { applyPinnedSummaryLanguageToMeeting } from '@/lib/summary-language-preferences';
import {
  EMPTY_DISK_SCAN,
  mergeRecoveryCandidates,
  readDiskScan,
  type DiskScan,
  type RecoveryCandidate,
} from '@/lib/interruptedRecordings';
import { toast } from 'sonner';
import { errorText } from '@/lib/errorText';

interface AudioRecoveryStatus {
  status: string; // "success" | "partial" | "failed" | "none"
  chunk_count: number;
  estimated_duration_seconds: number;
  audio_file_path?: string;
  message: string;
}

/**
 * Every interrupted recording known right now: the browser's unsaved meetings
 * (with their audio checked) merged with what the core finds on disk, one entry
 * per folder. A failed disk scan leaves the browser list as it was.
 */
export async function fetchRecoveryCandidates(
  options: { includeKept?: boolean } = {},
): Promise<{ candidates: RecoveryCandidate[]; scan: DiskScan; scanFailed: boolean; keptCount: number }> {
  const meetings = await indexedDBService.getAllMeetings().catch((error) => {
    console.error('Failed to read the browser recovery data:', error);
    return [] as MeetingMetadata[];
  });

  // Filter out meetings older than 7 days and newer than 15 seconds
  // The 15 seconds threshold prevents showing meetings from the current session(jus in case)
  // where recording just stopped but hasn't been fully saved yet
  const cutoffTime = Date.now() - (7 * 24 * 60 * 60 * 1000);
  const secondsAgo = Date.now() - (2 * 1000);

  const recentMeetings = meetings.filter(m => {
    const isWithinRetention = m.lastUpdated > cutoffTime; // Not older than 7 days
    const isOldEnough = m.lastUpdated < secondsAgo; // Older than 15 seconds
    return isWithinRetention && isOldEnough;
  });

  let scan = EMPTY_DISK_SCAN;
  let scanFailed = false;
  try {
    scan = readDiskScan(await invoke<unknown>('find_interrupted_recordings', {
      cacheIds: recentMeetings.map(meeting => meeting.meetingId),
    }));
  } catch (error) {
    console.warn('Could not look for interrupted recordings on disk:', error);
    scanFailed = true;
  }
  const resolvedCacheIds = new Set(scan.resolvedCacheIds);
  const pendingMeetings = recentMeetings.filter(meeting => !resolvedCacheIds.has(meeting.meetingId));

  // Verify audio checkpoint availability for each pending meeting.
  const meetingsWithAudioStatus = await Promise.all(
    pendingMeetings.map(async (meeting) => {
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
        } catch (error) {
          console.warn('Failed to check audio for meeting:', error);
          // On error, assume no audio to be safe
          return { ...meeting, recordingFolder: meeting.folderPath, folderPath: undefined };
        }
      }
      return meeting;
    })
  );

  const merged = mergeRecoveryCandidates(meetingsWithAudioStatus, scan.recordings);
  // A recording the user asked not to be asked about again is left out, and
  // counted, so Settings can still offer to show it.
  const keptCount = merged.filter(candidate => candidate.disk?.kept).length;
  const candidates = options.includeKept ? merged : merged.filter(candidate => !candidate.disk?.kept);
  return { candidates, scan, scanFailed, keptCount };
}

export interface UseTranscriptRecoveryReturn {
  recoverableMeetings: RecoveryCandidate[];
  isLoading: boolean;
  isRecovering: boolean;
  /** The recordings found on disk could be incomplete: the scan stopped at a bound. */
  diskScanTruncated: boolean;
  checkForRecoverableTranscripts: (options?: { includeKept?: boolean }) => Promise<RecoveryCandidate[]>;
  /** "Keep, do not ask again": the recording stays on disk and is not offered unasked. */
  keepRecoverableMeeting: (meetingId: string) => Promise<void>;
  recoverMeeting: (meetingId: string) => Promise<{ success: boolean; audioRecoveryStatus?: AudioRecoveryStatus | null; meetingId?: string; transcriptCount?: number }>;
  loadMeetingTranscripts: (meetingId: string) => Promise<TranscriptPreview[]>;
  deleteRecoverableMeeting: (meetingId: string) => Promise<void>;
}

export function useTranscriptRecovery(): UseTranscriptRecoveryReturn {
  const [recoverableMeetings, setRecoverableMeetings] = useState<RecoveryCandidate[]>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [isRecovering, setIsRecovering] = useState(false);
  const [diskScanTruncated, setDiskScanTruncated] = useState(false);
  const candidatesRef = useRef<RecoveryCandidate[]>([]);

  const setCandidates = useCallback((next: RecoveryCandidate[] | ((prev: RecoveryCandidate[]) => RecoveryCandidate[])) => {
    candidatesRef.current = typeof next === 'function' ? next(candidatesRef.current) : next;
    setRecoverableMeetings(candidatesRef.current);
  }, []);

  /**
   * Check for recoverable meetings: unsaved meetings in IndexedDB and
   * interrupted recordings found in the recording folders.
   */
  const checkForRecoverableTranscripts = useCallback(async (options: { includeKept?: boolean } = {}): Promise<RecoveryCandidate[]> => {
    setIsLoading(true);
    try {
      const { candidates, scan } = await fetchRecoveryCandidates(options);
      setCandidates(candidates);
      setDiskScanTruncated(scan.truncated);
      return candidates;
    } catch (error) {
      console.error('Failed to check for recoverable transcripts:', error);
      setCandidates([]);
      return [];
    } finally {
      setIsLoading(false);
    }
  }, [setCandidates]);

  /**
   * Load transcripts for preview
   */
  const loadMeetingTranscripts = useCallback(async (meetingId: string): Promise<TranscriptPreview[]> => {
    try {
      const candidate = candidatesRef.current.find(c => c.meetingId === meetingId);
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
    } catch (error) {
      console.error('Failed to load meeting transcripts:', error);
      return [];
    }
  }, []);

  /**
   * The steps after a recovered meeting is saved: language, originals and the list.
   */
  const finishRecovery = useCallback(async (
    entryId: string,
    folderPath: string | undefined,
    audioRecoveryStatus: AudioRecoveryStatus | null,
    savedMeetingId: string,
  ) => {
    const audioRecovered = Boolean(audioRecoveryStatus?.audio_file_path) &&
      (audioRecoveryStatus?.status === 'success' || audioRecoveryStatus?.status === 'partial');
    try {
      await applyPinnedSummaryLanguageToMeeting(savedMeetingId);
    } catch (error) {
      console.warn('Failed to apply pinned summary language to recovered meeting:', error);
      toast.warning('Could not apply default summary language', {
        description: 'The recovered meeting was saved, but the default summary language was not applied.',
      });
    }
    return { audioRecovered, entryId, folderPath };
  }, []);

  /**
   * Recover a recording that exists only on disk. Audio goes through the same
   * recovery as before; the transcript comes from the snapshot the core wrote
   * next to it, and the meeting is saved through the normal save path, which
   * registers it and marks its folder.
   */
  const recoverDiskMeeting = useCallback(async (candidate: RecoveryCandidate) => {
    const recording = candidate.disk!;
    // A recording that already belongs to a library meeting repairs that meeting
    // (the core recovers the audio into its folder); it is never saved again.
    if (recording.registeredMeetingId) {
      const repaired = await invoke<{ meetingId: string; audioStatus: string; audioPath?: string | null }>(
        'repair_saved_recording',
        { folder: recording.folder },
      );
      setCandidates(prev => prev.filter(m => m.meetingId !== candidate.meetingId));
      return {
        success: true,
        audioRecoveryStatus: {
          status: repaired.audioStatus,
          chunk_count: 0,
          estimated_duration_seconds: 0,
          audio_file_path: repaired.audioPath ?? undefined,
          message: '',
        } as AudioRecoveryStatus,
        meetingId: repaired.meetingId,
        transcriptCount: 0,
      };
    }
    let audioRecoveryStatus: AudioRecoveryStatus | null = null;
    if (recording.hasAudio) {
      try {
        audioRecoveryStatus = await invoke<AudioRecoveryStatus>('recover_audio_from_checkpoints', { meetingFolder: recording.folder });
      } catch (error) {
        console.error('Audio recovery failed:', error);
        audioRecoveryStatus = {
          status: 'failed',
          chunk_count: 0,
          estimated_duration_seconds: 0,
          message: errorText(error),
        };
      }
    } else {
      audioRecoveryStatus = { status: 'none', chunk_count: 0, estimated_duration_seconds: 0, message: 'No audio on disk' };
    }
    const audioRecovered = Boolean(audioRecoveryStatus?.audio_file_path) &&
      (audioRecoveryStatus?.status === 'success' || audioRecoveryStatus?.status === 'partial');
    if (!recording.hasTranscript && !audioRecovered) {
      throw new Error('No audio could be recovered yet. Keep the recording folder and retry recovery; a transcript is not required.');
    }
    const saveResponse = await storageService.saveMeeting(
      candidate.title,
      [],
      recording.folder,
      {
        audio_save_failed: audioRecoveryStatus?.status === 'failed',
        transcription_incomplete: recording.hasTranscript && !recording.reasons.includes('library_handoff_pending'),
        capture_incomplete: audioRecoveryStatus?.status === 'partial',
      },
      // The core reads the transcript snapshot from the folder.
      recording.hasTranscript,
    );
    const savedMeetingId = saveResponse.meeting_id;
    const { audioRecovered: recovered } = await finishRecovery(candidate.meetingId, recording.folder, audioRecoveryStatus, savedMeetingId);
    if (recovered) {
      try {
        await invoke('release_recovered_capture', { meetingFolder: recording.folder });
      } catch {
        console.warn('Meeting saved; recovery originals retained because cleanup could not finish');
      }
      if (audioRecoveryStatus?.status === 'success') {
        try {
          await invoke('cleanup_checkpoints', { meetingFolder: recording.folder });
        } catch (error) {
          console.warn('Checkpoint cleanup failed (non-fatal):', error);
        }
      }
    }
    setCandidates(prev => prev.filter(m => m.meetingId !== candidate.meetingId));
    return {
      success: true,
      audioRecoveryStatus,
      meetingId: savedMeetingId,
      transcriptCount: recording.transcriptLines,
    };
  }, [finishRecovery, setCandidates]);

  /**
   * Recover a meeting from IndexedDB
   */
  const recoverMeeting = useCallback(async (meetingId: string): Promise<{ success: boolean; audioRecoveryStatus?: AudioRecoveryStatus | null; meetingId?: string; transcriptCount?: number }> => {
    setIsRecovering(true);
    try {
      const diskOnly = candidatesRef.current.find(c => c.meetingId === meetingId && c.source === 'disk');
      if (diskOnly?.disk) {
        return await recoverDiskMeeting(diskOnly);
      }
      // 1. Load meeting metadata
      const metadata = await indexedDBService.getMeetingMetadata(meetingId);
      if (!metadata) {
        throw new Error('Meeting metadata not found');
      }

      // 2. Load all transcripts
      // A failed IndexedDB read is different from a deliberately empty transcript.
      const transcripts = await indexedDBService.getTranscripts(meetingId);

      // Recovery only uses the folder recorded for this entry.
      const folderPath = metadata.folderPath;

      // 4. Attempt audio recovery if folder path exists
      let audioRecoveryStatus: AudioRecoveryStatus | null = null;
      if (folderPath) {
        try {
          audioRecoveryStatus = await invoke<AudioRecoveryStatus>(
            'recover_audio_from_checkpoints',
            { meetingFolder: folderPath }
          );
        } catch (error) {
          console.error('Audio recovery failed:', error);
          audioRecoveryStatus = {
            status: 'failed',
            chunk_count: 0,
            estimated_duration_seconds: 0,
            message: errorText(error)
          };
        }
      } else {
        audioRecoveryStatus = {
          status: 'none',
          chunk_count: 0,
          estimated_duration_seconds: 0,
          message: 'No folder path available'
        };
      }

      // Audio-only recordings have no transcript by design. Recover the audio
      // first, and keep IndexedDB plus all originals if nothing was recovered.
      const audioRecovered = Boolean(audioRecoveryStatus?.audio_file_path) &&
        (audioRecoveryStatus?.status === 'success' || audioRecoveryStatus?.status === 'partial');
      if (transcripts.length === 0 && !audioRecovered) {
        throw new Error('No audio could be recovered yet. Keep the recording folder and retry recovery; a transcript is not required.');
      }

      // 5. Convert StoredTranscripts to the format expected by storageService
      const formattedTranscripts = transcripts.map((t, index) => ({
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

      // 6. Save to backend database using existing save utilities
      const saveResponse = await storageService.saveMeeting(
        metadata.title,
        formattedTranscripts,
        folderPath ?? null,
        {
          audio_save_failed: audioRecoveryStatus?.status === 'failed',
          transcription_incomplete: transcripts.length > 0,
          capture_incomplete: audioRecoveryStatus?.status === 'partial',
        },
      );

      const savedMeetingId = saveResponse.meeting_id;

      try {
        await applyPinnedSummaryLanguageToMeeting(savedMeetingId);
      } catch (error) {
        console.warn('Failed to apply pinned summary language to recovered meeting:', error);
        toast.warning('Could not apply default summary language', {
          description: 'The recovered meeting was saved, but the default summary language was not applied.',
        });
      }

      // 7. Mark as saved in IndexedDB
      await indexedDBService.markMeetingSaved(meetingId);


      if (folderPath && audioRecovered) {
        try {
          await invoke('release_recovered_capture', { meetingFolder: folderPath });
        } catch {
          console.warn('Meeting saved; recovery originals retained because cleanup could not finish');
        }
      }

      // 8. Only remove legacy checkpoints once all their audio was recovered.
      // Failed/partial recovery must retain originals for another attempt.
      if (folderPath && audioRecovered && audioRecoveryStatus?.status === 'success') {
        try {
          await invoke('cleanup_checkpoints', { meetingFolder: folderPath });
        } catch (error) {
          // Non-fatal - don't fail recovery if cleanup fails
          console.warn('Checkpoint cleanup failed (non-fatal):', error);
        }
      }

      // 9. Remove from recoverable list
      setCandidates(prev => prev.filter(m => m.meetingId !== meetingId));

      return {
        success: true,
        audioRecoveryStatus,
        meetingId: savedMeetingId,
        transcriptCount: transcripts.length,
      };
    } catch (error) {
      console.error('Failed to recover meeting:', error);
      throw error;
    } finally {
      setIsRecovering(false);
    }
  }, [recoverDiskMeeting, setCandidates]);

  /**
   * Delete a recoverable meeting. A recording that exists only on disk is
   * deleted by the core (validated, only an unsaved interrupted folder), and
   * only because the user asked; nothing is ever deleted automatically.
   */
  const deleteRecoverableMeeting = useCallback(async (meetingId: string): Promise<void> => {
    try {
      const candidate = candidatesRef.current.find(c => c.meetingId === meetingId);
      if (candidate?.source === 'disk' && candidate.disk) {
        if (candidate.disk.registeredMeetingId) {
          throw new Error('This recording is saved in your library and cannot be deleted from here.');
        }
        await invoke('discard_interrupted_recording', { folder: candidate.disk.folder });
      } else {
        await indexedDBService.deleteMeeting(meetingId);
      }
      setCandidates(prev => prev.filter(m => m.meetingId !== meetingId));
    } catch (error) {
      console.error('Failed to delete meeting:', error);
      throw error;
    }
  }, [setCandidates]);

  const keepRecoverableMeeting = useCallback(async (meetingId: string): Promise<void> => {
    const candidate = candidatesRef.current.find(c => c.meetingId === meetingId);
    const folder = candidate?.disk?.folder;
    if (!folder) throw new Error('This recording is not on disk and cannot be kept this way.');
    await invoke('keep_interrupted_recording', { folder, keep: true });
    setCandidates(prev => prev.filter(m => m.meetingId !== meetingId));
  }, [setCandidates]);

  return {
    recoverableMeetings,
    isLoading,
    isRecovering,
    diskScanTruncated,
    checkForRecoverableTranscripts,
    keepRecoverableMeeting,
    recoverMeeting,
    loadMeetingTranscripts,
    deleteRecoverableMeeting
  };
}
