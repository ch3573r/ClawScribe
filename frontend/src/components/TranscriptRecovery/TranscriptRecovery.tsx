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
import { describeDuration, describeSize, type RecoveryCandidate } from '@/lib/interruptedRecordings';
import { cn } from '@/lib/utils';

interface TranscriptRecoveryProps {
  isOpen: boolean;
  onClose: () => void;
  recoverableMeetings: RecoveryCandidate[];
  onRecover: (meetingId: string) => Promise<unknown>;
  onDelete: (meetingId: string) => Promise<void>;
  /** "Keep, do not ask again" for a recording found on disk. */
  onKeep?: (meetingId: string) => Promise<void>;
  onLoadPreview: (meetingId: string) => Promise<TranscriptPreview[]>;
}

export function TranscriptRecovery({
  isOpen,
  onClose,
  recoverableMeetings,
  onRecover,
  onDelete,
  onKeep,
  onLoadPreview,
}: TranscriptRecoveryProps) {
  const [requestedMeetingId, setRequestedMeetingId] = useState<string | null>(null);
  const selectedMeeting = recoverableMeetings.find(m => m.meetingId === requestedMeetingId) ?? recoverableMeetings[0];
  const selectedMeetingId = selectedMeeting?.meetingId ?? null;
  const [preview, setPreview] = useState<{ meetingId: string; rows: TranscriptPreview[] } | null>(null);
  const isLoadingPreview = !!selectedMeetingId && preview?.meetingId !== selectedMeetingId;
  const previewTranscripts = preview?.meetingId === selectedMeetingId ? preview.rows : [];
  const [isRecovering, setIsRecovering] = useState(false);
  const [isDeleting, setIsDeleting] = useState(false);
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
      if (active) setPreview({ meetingId: selectedMeetingId, rows: transcripts.slice(0, 10) });
    }).catch(error => {
      if (!active) return;
      console.error('Failed to load preview:', error);
      setPreview({ meetingId: selectedMeetingId, rows: [] });
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

  return (
    <Dialog open={isOpen} onOpenChange={open => { if (!open && !isRecovering && !isDeleting) onClose(); }}>
      <DialogContent className="max-w-4xl h-[80vh] flex flex-col p-0">
        <DialogHeader className="px-6 pt-6">
          <DialogTitle className="text-2xl">Recover interrupted meetings</DialogTitle>
          <DialogDescription>
            We found {recoverableMeetings.length} meeting{recoverableMeetings.length !== 1 ? 's' : ''} that {recoverableMeetings.length !== 1 ? 'were' : 'was'} interrupted. Select a meeting to preview and recover it.
          </DialogDescription>
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
                    disabled={isRecovering || isDeleting}
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
                      {meeting.folderPath ? (
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
                      {selectedMeeting.folderPath ? (
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
                        No transcript yet. A transcript is not required to recover recorded audio.
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
            disabled={isRecovering || isDeleting}
          >
            Cancel
          </Button>
          {onKeep && selectedMeeting?.disk && (
            <Button variant="ghost" onClick={handleKeep} disabled={isRecovering || isDeleting}>
              Keep, do not ask again
            </Button>
          )}
          <Button
            ref={deleteButtonRef}
            variant="outline"
            className="text-destructive"
            onClick={() => setDeleteTargetId(selectedMeetingId)}
            disabled={!selectedMeetingId || isRecovering || isDeleting || deleteBlocked}
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
            disabled={!selectedMeetingId || isRecovering || isDeleting}
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
