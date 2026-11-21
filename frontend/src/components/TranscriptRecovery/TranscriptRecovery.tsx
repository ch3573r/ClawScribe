/**
 * TranscriptRecovery Component
 *
 * Modal dialog for recovering interrupted meetings from IndexedDB.
 * Displays recoverable meetings, allows preview, and enables recovery or deletion.
 */

import React, { useState, useEffect, useRef } from 'react';
import { toast } from 'sonner';
import { formatDistanceToNow } from 'date-fns';
import { AlertCircle, CheckCircle2, Clock, FileText, Trash2, XCircle } from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Alert, AlertDescription } from '@/components/ui/alert';
import type { TranscriptPreview } from '@/services/indexedDBService';
import { errorText } from '@/lib/errorText';
import { describeDuration, describeSize, hasStableRecordingId, type OfflineTranscriptSaveResult, type RecoveryCandidate } from '@/lib/interruptedRecordings';
import { cn } from '@/lib/utils';

interface TranscriptRecoveryProps {
  isOpen: boolean;
  onClose: () => void;
  recoverableMeetings: RecoveryCandidate[];
  isScanning: boolean;
  scanFailed: boolean;
  hasUnverifiedFolders: boolean;
  onRetryScan: () => void;
  onRecover: (meetingId: string) => Promise<unknown>;
  onDelete: (meetingId: string) => Promise<void>;
  /** "Keep, do not ask again" for a recording found on disk. */
  onKeep?: (meetingId: string) => Promise<void>;
  onLoadPreview: (meetingId: string) => Promise<TranscriptPreview[]>;
  onSaveOfflineTranscript: (meetingId: string) => Promise<OfflineTranscriptSaveResult>;
}

export function TranscriptRecovery({
  isOpen,
  onClose,
  recoverableMeetings,
  isScanning,
  scanFailed,
  hasUnverifiedFolders,
  onRetryScan,
  onRecover,
  onDelete,
  onKeep,
  onLoadPreview,
  onSaveOfflineTranscript,
}: TranscriptRecoveryProps) {
  const [requestedMeetingId, setRequestedMeetingId] = useState<string | null>(null);
  const selectedMeeting = recoverableMeetings.find(m => m.meetingId === requestedMeetingId) ?? recoverableMeetings[0];
  const selectedMeetingId = selectedMeeting?.meetingId ?? null;
  const [preview, setPreview] = useState<{ meetingId: string; rows: TranscriptPreview[]; failed?: boolean; hasTranscript?: boolean } | null>(null);
  const isLoadingPreview = !!selectedMeetingId && preview?.meetingId !== selectedMeetingId;
  const previewTranscripts = preview?.meetingId === selectedMeetingId ? preview.rows : [];
  const [isRecovering, setIsRecovering] = useState(false);
  const [isDeleting, setIsDeleting] = useState(false);
  const [isSavingTranscript, setIsSavingTranscript] = useState(false);
  const savingTranscript = useRef(false);
  const busy = isRecovering || isDeleting || isSavingTranscript;
  const [deleteTargetId, setDeleteTargetId] = useState<string | null>(null);
  const cancelDeleteRef = useRef<HTMLButtonElement>(null);
  const deleteButtonRef = useRef<HTMLButtonElement>(null);

  // Reopen on the first available meeting.
  useEffect(() => {
    setDeleteTargetId(null);
    if (!isOpen) {
      setRequestedMeetingId(null);
      setPreview(null);
    }
  }, [isOpen]);

  useEffect(() => {
    if (!isOpen || !selectedMeetingId) return;
    let active = true;
    void onLoadPreview(selectedMeetingId).then(transcripts => {
      if (active) setPreview({
        meetingId: selectedMeetingId,
        rows: transcripts.slice(0, 10),
        hasTranscript: transcripts.some(row => row.text.trim()),
      });
    }).catch(() => {
      if (active) setPreview({ meetingId: selectedMeetingId, rows: [], failed: true });
    });
    return () => { active = false; };
  }, [isOpen, selectedMeetingId, onLoadPreview]);

  const handleRecover = async () => {
    if (!selectedMeetingId) return;

    setIsRecovering(true);
    try {
      await onRecover(selectedMeetingId);
      onClose();
    } catch (error) {
      console.error('Meeting recovery failed:', error);
      toast.error('Could not recover the meeting. Your recovery data is still available. Try again.');
    } finally {
      setIsRecovering(false);
    }
  };

  const handleSaveTranscript = async () => {
    if (!selectedMeetingId || busy || savingTranscript.current) return;
    savingTranscript.current = true;
    setIsSavingTranscript(true);
    try {
      const result = await onSaveOfflineTranscript(selectedMeetingId);
      if (result === 'library') {
        toast.success('Transcript saved to your library');
        onClose();
      } else if (result === 'file') {
        toast.success('Transcript copy saved');
      }
    } catch (error) {
      toast.error(errorText(error, 'Could not save the transcript. Your draft is still available.'));
    } finally {
      savingTranscript.current = false;
      setIsSavingTranscript(false);
    }
  };

  const handleKeep = async () => {
    if (!selectedMeetingId || !onKeep || isRecovering || isDeleting) return;
    try {
      await onKeep(selectedMeetingId);
      setRequestedMeetingId(null);
      setPreview(null);
    } catch (error) {
      console.error('Keeping the recording failed:', error);
      toast.error(errorText(error, 'Could not keep the recording. Try again.'));
    }
  };

  const handleDelete = async () => {
    if (!deleteTargetId || isDeleting || isRecovering) return;

    setIsDeleting(true);
    try {
      await onDelete(deleteTargetId);
      setDeleteTargetId(null);
      setRequestedMeetingId(null);
      setPreview(null);
    } catch (error) {
      console.error('Meeting recovery deletion failed:', error);
      toast.error('Could not delete the meeting. Try again.');
    } finally {
      setIsDeleting(false);
    }
  };

  // A recording found on disk says why it counts as interrupted. One that is
  // already saved in the library cannot be deleted from here.
  const diskDetails = selectedMeeting?.disk
    ? [describeDuration(selectedMeeting.disk.approxDurationSeconds), describeSize(selectedMeeting.disk.sizeBytes)].filter(Boolean).join(' · ')
    : '';
  const deleteTarget = recoverableMeetings.find(m => m.meetingId === deleteTargetId);
  const deleteBlocked = !!selectedMeeting?.disk?.registeredMeetingId;
  const folderUnverified = selectedMeeting?.folderUnverified === true;
  const canSaveToLibrary = folderUnverified && hasStableRecordingId(selectedMeeting.meetingId);

  if (isScanning || recoverableMeetings.length === 0) {
    return (
      <Dialog open={isOpen} onOpenChange={open => { if (!open) onClose(); }}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Interrupted recordings</DialogTitle>
            <DialogDescription role={scanFailed && !isScanning ? 'alert' : 'status'}>
              {isScanning ? 'Checking for interrupted recordings…' : scanFailed
                ? 'The recovery scan failed. Try again. Your drafts are kept.'
                : 'No interrupted recordings found.'}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={onClose}>Close</Button>
            {scanFailed && !isScanning && <Button onClick={onRetryScan}>Retry scan</Button>}
          </DialogFooter>
        </DialogContent>
      </Dialog>
    );
  }

  return (
    <Dialog open={isOpen} onOpenChange={open => { if (!open && !busy) onClose(); }}>
      <DialogContent className="max-w-4xl h-[80vh] flex flex-col p-0">
        <DialogHeader className="px-6 pt-6">
          <DialogTitle className="text-2xl">Recover interrupted meetings</DialogTitle>
          <DialogDescription>
            We found {recoverableMeetings.length} meeting{recoverableMeetings.length !== 1 ? 's' : ''} that {recoverableMeetings.length !== 1 ? 'were' : 'was'} interrupted. Select a meeting to preview and recover it.
          </DialogDescription>
          {(scanFailed || hasUnverifiedFolders) && (
            <div className="flex items-center justify-between gap-3">
              <p role="alert" className="text-sm text-muted-foreground">
                {scanFailed ? 'The recovery scan failed. Your drafts are kept.' : 'Some folders could not be verified. Reconnect the recordings location if needed, then retry.'}
              </p>
              <Button variant="outline" size="sm" onClick={onRetryScan} disabled={busy}>Retry scan</Button>
            </div>
          )}
        </DialogHeader>

        <div className="flex-1 flex gap-4 px-6 pb-6 overflow-hidden">
          {/* Meeting List */}
          <div className="w-1/3 flex flex-col">
            <h3 className="text-sm font-medium mb-2">Interrupted meetings</h3>
            <ScrollArea className="flex-1 border rounded-lg">
              <div className="p-2 space-y-2">
                {recoverableMeetings.map((meeting) => (
                  <Button variant="ghost"
                    key={meeting.meetingId}
                    disabled={busy}
                    onClick={() => setRequestedMeetingId(meeting.meetingId)}
                    className={cn(
                      'block h-auto font-normal text-base whitespace-normal hover:text-foreground w-full text-left p-3 rounded-lg border transition-colors',
                      selectedMeetingId === meeting.meetingId
                        ? 'bg-primary/10 hover:bg-primary/10 border-primary'
                        : 'hover:bg-muted border-transparent'
                    )}
                  >
                    <div className="flex items-start justify-between gap-2">
                      <div className="flex-1 min-w-0">
                        <p className="font-medium text-sm truncate">{meeting.title}</p>
                        <p className="text-xs text-muted-foreground flex items-center gap-1 mt-1">
                          <Clock className="!w-3 !h-3" />
                          {formatDistanceToNow(new Date(meeting.lastUpdated), { addSuffix: true })}
                        </p>
                        <p className="text-xs text-muted-foreground flex items-center gap-1 mt-1">
                          <FileText className="!w-3 !h-3" />
                          {meeting.transcriptCount} transcript{meeting.transcriptCount !== 1 ? 's' : ''}
                        </p>
                      </div>
                      {meeting.folderUnverified ? (
                        <span title="Folder unverified">
                          <AlertCircle className="w-4 h-4 text-warning-foreground flex-shrink-0" />
                        </span>
                      ) : meeting.folderPath ? (
                        <span title="Audio available">
                          <CheckCircle2 className="w-4 h-4 text-success-foreground flex-shrink-0" />
                        </span>
                      ) : (
                        <span title="No audio">
                          <AlertCircle className="w-4 h-4 text-warning-foreground flex-shrink-0" />
                        </span>
                      )}
                    </div>
                  </Button>
                ))}
              </div>
            </ScrollArea>
          </div>

          {/* Preview Panel */}
          <div className="flex-1 flex flex-col">
            <h3 className="text-sm font-medium mb-2">Preview</h3>
            <div className="flex-1 border rounded-lg overflow-hidden flex flex-col">
              {selectedMeeting ? (
                <>
                  {/* Meeting Info */}
                  <div className="p-4 border-b bg-muted/50">
                    <h4 className="font-semibold">{selectedMeeting.title}</h4>
                    <p className="text-sm text-muted-foreground mt-1">
                      Started {new Date(selectedMeeting.startTime).toLocaleString()}
                    </p>
                    <div className="flex items-center gap-4 mt-2 text-sm">
                      <span className="flex items-center gap-1">
                        <FileText className="w-4 h-4" />
                        {selectedMeeting.transcriptCount} transcripts
                      </span>
                      {folderUnverified ? (
                        <span className="flex items-center gap-1 text-warning-foreground">
                          <AlertCircle className="w-4 h-4" />
                          Folder unverified
                        </span>
                      ) : selectedMeeting.folderPath ? (
                        <span className="flex items-center gap-1 text-success-foreground">
                          <CheckCircle2 className="w-4 h-4" />
                          Audio available
                        </span>
                      ) : (
                        <span className="flex items-center gap-1 text-warning-foreground">
                          <AlertCircle className="w-4 h-4" />
                          No audio
                        </span>
                      )}
                    </div>
                    {folderUnverified && preview?.meetingId === selectedMeetingId && preview?.hasTranscript && (
                      <p className="text-sm text-muted-foreground mt-2">
                        {canSaveToLibrary
                          ? 'Save the full transcript to your library now. When the folder is verified, use recovery to add its audio to the same meeting.'
                          : 'Save the full transcript as a text file. The original draft and its folder are kept for a future scan.'}
                      </p>
                    )}
                    {selectedMeeting.disk && (
                      <p className="text-sm text-muted-foreground mt-2">
                        {selectedMeeting.disk.reason}{diskDetails ? ` (${diskDetails})` : ''}
                        {deleteBlocked ? '. It is saved in your library.' : ''}
                      </p>
                    )}
                  </div>

                  {/* Transcript Preview */}
                  <ScrollArea className="flex-1 p-4">
                    {isLoadingPreview ? (
                      <div className="flex items-center justify-center h-full text-muted-foreground">
                        Loading preview...
                      </div>
                    ) : preview?.meetingId === selectedMeetingId && preview?.failed ? (
                      <p role="alert" className="text-sm text-muted-foreground">Could not load the transcript preview. Check the recording again.</p>
                    ) : previewTranscripts.length > 0 ? (
                      <div className="space-y-3">
                        <Alert>
                          <AlertDescription>
                            Showing first {previewTranscripts.length} transcript segments (of {selectedMeeting.transcriptCount} total)
                          </AlertDescription>
                        </Alert>
                        {previewTranscripts.map((transcript, index) => {
                          // Handle different timestamp formats
                          const getTimestamp = () => {
                            if (!transcript.timestamp) return '--:--';
                            try {
                              const date = new Date(transcript.timestamp);
                              if (isNaN(date.getTime())) {
                                // If timestamp is invalid, try audio_start_time
                                if (transcript.audio_start_time !== undefined) {
                                  const totalSecs = Math.floor(transcript.audio_start_time);
                                  const mins = Math.floor(totalSecs / 60);
                                  const secs = totalSecs % 60;
                                  return `${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
                                }
                                return '--:--';
                              }
                              return date.toLocaleTimeString();
                            } catch {
                              return '--:--';
                            }
                          };

                          return (
                            <div key={index} className="text-sm">
                              <span className="text-muted-foreground">[{getTimestamp()}]</span>{' '}
                              <span>{transcript.text}</span>
                            </div>
                          );
                        })}
                        {selectedMeeting.transcriptCount > 10 && (
                          <p className="text-sm text-muted-foreground italic">
                            ... and {selectedMeeting.transcriptCount - 10} more transcript{selectedMeeting.transcriptCount - 10 !== 1 ? 's' : ''}
                          </p>
                        )}
                      </div>
                    ) : (
                      <div className="flex items-center justify-center h-full text-muted-foreground">
                        {folderUnverified ? 'There is no cached transcript.' : 'No transcript yet. You can still recover recorded audio.'}
                      </div>
                    )}
                  </ScrollArea>
                </>
              ) : (
                <div className="flex items-center justify-center h-full text-muted-foreground">
                  Select a meeting to preview
                </div>
              )}
            </div>
          </div>
        </div>

        <DialogFooter className="px-6 pb-6">
          <Button
            variant="outline"
            onClick={onClose}
            disabled={busy}
          >
            Cancel
          </Button>
          {onKeep && selectedMeeting?.disk && (
            <Button variant="ghost" onClick={handleKeep} disabled={busy}>
              Keep, do not ask again
            </Button>
          )}
          {folderUnverified && (isLoadingPreview || preview?.hasTranscript) && (
            <Button onClick={handleSaveTranscript} disabled={busy || isLoadingPreview}>
              {isSavingTranscript ? 'Saving transcript…' : canSaveToLibrary ? 'Save transcript to library' : 'Save transcript copy…'}
            </Button>
          )}
          {!folderUnverified && (
            <>
              <Button
                ref={deleteButtonRef}
                variant="outline"
                className="text-destructive"
                onClick={() => setDeleteTargetId(selectedMeetingId)}
                disabled={!selectedMeetingId || busy || deleteBlocked}
              >
                {isDeleting ? (
                  <>
                    <XCircle className="w-4 h-4 mr-2 animate-spin" />
                    Deleting...
                  </>
                ) : (
                  <>
                    <Trash2 className="w-4 h-4 mr-2" />
                    Delete
                  </>
                )}
              </Button>
              <Button
                onClick={handleRecover}
                disabled={!selectedMeetingId || busy}
              >
                {isRecovering ? (
                  <>
                    <CheckCircle2 className="w-4 h-4 mr-2 animate-spin" />
                    Recovering...
                  </>
                ) : (
                  <>
                    <CheckCircle2 className="w-4 h-4 mr-2" />
                    Recover
                  </>
                )}
              </Button>
            </>
          )}
        </DialogFooter>
        <Dialog open={deleteTargetId !== null} onOpenChange={open => { if (!open && !isDeleting) setDeleteTargetId(null); }}>
          <DialogContent
            onOpenAutoFocus={event => { event.preventDefault(); cancelDeleteRef.current?.focus(); }}
            onCloseAutoFocus={event => { event.preventDefault(); deleteButtonRef.current?.focus(); }}
          >
            <DialogHeader>
              <DialogTitle>Delete interrupted meeting?</DialogTitle>
              <DialogDescription>
                {deleteTarget?.source === 'disk'
                  ? "This deletes the recording folder, including its audio, from this PC. This cannot be undone."
                  : "This deletes the selected meeting's recovery data. This cannot be undone."}
              </DialogDescription>
            </DialogHeader>
            <DialogFooter>
              <Button ref={cancelDeleteRef} variant="outline" onClick={() => setDeleteTargetId(null)} disabled={isDeleting}>Cancel</Button>
              <Button variant="destructive" onClick={handleDelete} disabled={isDeleting}>{isDeleting ? "Deleting…" : "Delete meeting"}</Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </DialogContent>
    </Dialog>
  );
}
