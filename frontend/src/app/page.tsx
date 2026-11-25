'use client';

import { useState, useEffect } from 'react';
import { motion } from 'framer-motion';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { usePermissionCheck } from '@/hooks/usePermissionCheck';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useTranscriptionConfig } from '@/contexts/ConfigContext';
import { useLiveAssistanceOwner } from '@/contexts/LiveAssistanceContext';
import { StatusOverlays } from '@/app/_components/StatusOverlays';
import Analytics from '@/lib/analytics';
import { SettingsModals } from './_components/SettingsModal';
import { TranscriptPanel } from './_components/TranscriptPanel';
import { LiveAssistanceDock } from '@/components/LiveAssistanceDock';
import { HomeDashboard } from '@/components/HomeDashboard';
import { RecordingControls } from '@/components/RecordingControls';
import { ActiveRecordingTitle } from './_components/ActiveRecordingTitle';
import { useModalState } from '@/hooks/useModalState';
import { useRecordingStart } from '@/hooks/useRecordingStart';
import { useTranscriptRecovery } from '@/hooks/useTranscriptRecovery';
import { RECOVERY_OPEN_ALL, RECOVERY_OPEN_REQUEST } from '@/lib/interruptedRecordings';
import { TranscriptRecovery } from '@/components/TranscriptRecovery';
import { toast } from 'sonner';
import { useRouter } from 'next/navigation';
import { LIVE_MODEL_FAILURE_MESSAGE, transcriptionSettingsPath } from '@/lib/start-error';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';

export default function Home() {
  // Local page state (not moved to contexts)
  const [showRecoveryDialog, setShowRecoveryDialog] = useState(false);

  // Use contexts for state management
  const { meetingTitle, transcripts } = useTranscripts();
  const { transcriptModelConfig, selectedDevices } = useTranscriptionConfig();
  const recordingState = useRecordingState();
  const liveAssistance = useLiveAssistanceOwner();

  // Extract status from global state
  const { status, isRecording, isStarting, isStopping, isProcessing, isSaving } = recordingState;

  // Hooks
  const { hasMicrophone } = usePermissionCheck();
  const { sidebarOffset, isSidebarResizing, refetchMeetings, liveReadiness, nextMeetingTitle, setNextMeetingTitle,
    setIsMeetingActive, recordingActionReason } = useSidebar();
  const { modals, messages, showModal, hideModal, modelStartFailure, dismissModelStartFailure } = useModalState(transcriptModelConfig);
  const isRecordingDisabled = isStarting || isStopping || isProcessing || isSaving;
  const { handleRecordingStart } = useRecordingStart(setIsMeetingActive);
  const startFromHome = async () => {
    if (recordingActionReason) return;
    if (await handleRecordingStart(nextMeetingTitle)) {
      setNextMeetingTitle(current => current === nextMeetingTitle ? '' : current);
    }
  };

  // Recovery hook
  const {
    recoverableMeetings,
    isLoading: isRecoveryScanning,
    scanFailed: recoveryScanFailed,
    hasUnverifiedFolders,
    checkForRecoverableTranscripts,
    keepRecoverableMeeting,
    recoverMeeting,
    loadMeetingTranscripts,
    saveOfflineTranscript,
    deleteRecoverableMeeting
  } = useTranscriptRecovery();

  const router = useRouter();

  useEffect(() => {
    // Track page view
    Analytics.trackPageView('home');
  }, []);

  // Startup recovery check
  useEffect(() => {
    const performStartupChecks = async () => {
      try {
        // Skip recovery check if currently recording or processing stop
        // This prevents the recovery dialog from showing when:
        if (recordingState.isRecording ||
          status === RecordingStatus.STARTING ||
          status === RecordingStatus.STOPPING ||
          status === RecordingStatus.PROCESSING_TRANSCRIPTS ||
          status === RecordingStatus.SAVING) {
          console.log('Skipping recovery check - recording in progress or processing');
          return;
        }

        // Recovery copies that have not been saved are never expired by age.
        await checkForRecoverableTranscripts();
      } catch (error) {
        console.error('Failed to perform startup checks:', error);
      }
    };

    performStartupChecks();
  }, [checkForRecoverableTranscripts, recordingState.isRecording, status]);

  // Settings > General can ask for the dialog again ("Interrupted recordings").
  // It opens only when there is something to recover and nothing is running.
  useEffect(() => {
    let requested: string | null = null;
    try {
      requested = sessionStorage.getItem(RECOVERY_OPEN_REQUEST);
      sessionStorage.removeItem(RECOVERY_OPEN_REQUEST);
    } catch {
      return;
    }
    if (!requested) return;
    // "Show kept" also lists the recordings the user asked not to be asked about.
    setShowRecoveryDialog(true);
    void checkForRecoverableTranscripts({ includeKept: requested === RECOVERY_OPEN_ALL });
  }, [checkForRecoverableTranscripts]);

  // Watch for recoverable meetings changes and show dialog once per session
  useEffect(() => {
    // Only show dialog if we have meetings and haven't shown it yet this session
    if (recoverableMeetings.length > 0 || recoveryScanFailed) {
      const shownThisSession = sessionStorage.getItem('recovery_dialog_shown');
      if (!shownThisSession) {
        setShowRecoveryDialog(true);
        sessionStorage.setItem('recovery_dialog_shown', 'true');
      }
    }
  }, [recoverableMeetings, recoveryScanFailed]);

  // Handle recovery with toast notifications and navigation
  const handleRecovery = async (meetingId: string) => {
    const result = await recoverMeeting(meetingId);

    if (result.success) {
      toast.success('Meeting recovered', {
        description: result.audioRecoveryStatus?.status === 'partial'
          ? 'Available audio recovered with gaps. You can transcribe it from the saved meeting.'
          : result.audioRecoveryStatus?.status === 'success'
            ? result.transcriptCount === 0
              ? 'Audio recovered. You can transcribe it from the saved meeting.'
              : 'Transcripts and audio recovered'
            : result.audioRecoveryStatus?.status === 'failed'
              ? 'Transcripts saved. Audio recovery needs a retry; original recording files were kept.'
              : 'Transcripts recovered (no audio available)',
        action: result.meetingId ? {
          label: 'View meeting',
          onClick: () => {
            router.push(`/meeting-details?id=${result.meetingId}`);
          }
        } : undefined,
        duration: 10000,
      });

      // Refresh sidebar to show the newly recovered meeting
      await refetchMeetings();

      if (result.remainingCount === 0) {
        setShowRecoveryDialog(false);
        sessionStorage.removeItem('recovery_dialog_shown');

        if (result.meetingId) {
          setTimeout(() => {
            router.push(`/meeting-details?id=${result.meetingId}`);
          }, 2000);
        }
      }
    }
  };

  // Handle dialog close - clear session flag if no meetings left
  const handleDialogClose = () => {
    setShowRecoveryDialog(false);
    // If user closes dialog and there are no more meetings, clear the flag
    // This allows the dialog to show again next session if new meetings appear
    if (recoverableMeetings.length === 0) {
      sessionStorage.removeItem('recovery_dialog_shown');
    }
  };

  // Computed values using global status
  const isProcessingStop = status === RecordingStatus.PROCESSING_TRANSCRIPTS || isProcessing;
  const isRecordingOrStarting = recordingState.isRecording || isStarting;

  return (
    <motion.div
      initial={{ opacity: 0, y: 20 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3, ease: 'easeOut' }}
      className="flex h-full flex-col bg-background"
    >
      {/* All Modals supported*/}
      <SettingsModals
        modals={modals}
        messages={messages}
        onClose={hideModal}
      />
      <Dialog open={modelStartFailure !== null} onOpenChange={open => { if (!open) dismissModelStartFailure(); }}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Live transcription couldn’t start</DialogTitle>
            <DialogDescription>{modelStartFailure && LIVE_MODEL_FAILURE_MESSAGE[modelStartFailure]}</DialogDescription>
          </DialogHeader>
          {recordingState.modeError && <p role="alert" className="text-sm text-destructive">{recordingState.modeError}</p>}
          <DialogFooter>
            <Button variant="outline" onClick={() => {
              dismissModelStartFailure();
              router.push(transcriptionSettingsPath(liveReadiness.data?.provider));
            }}>Transcription settings</Button>
            <Button disabled={recordingState.isSavingMode} onClick={async () => {
              if (await recordingState.setRecordingMode('audio_only')) dismissModelStartFailure();
            }}>Use audio-only mode</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Recovery Dialog */}
      <TranscriptRecovery
        isOpen={showRecoveryDialog && !isRecording && !isRecordingDisabled}
        onClose={handleDialogClose}
        recoverableMeetings={recoverableMeetings}
        isScanning={isRecoveryScanning}
        scanFailed={recoveryScanFailed}
        hasUnverifiedFolders={hasUnverifiedFolders}
        onRetryScan={() => { void checkForRecoverableTranscripts(); }}
        onRecover={handleRecovery}
        onDelete={deleteRecoverableMeeting}
        onKeep={keepRecoverableMeeting}
        onLoadPreview={loadMeetingTranscripts}
        onSaveOfflineTranscript={saveOfflineTranscript}
      />
      <div className="flex flex-1 overflow-hidden">
        {isRecordingOrStarting || transcripts.length > 0 || isProcessingStop || isStopping ? (
          <>
            <div className="flex min-h-0 min-w-0 flex-1 flex-col">
              {isRecording && recordingState.sessionId && !isRecordingDisabled && (
                <div className="flex shrink-0 justify-center border-b border-border bg-card px-4 py-2">
                  <ActiveRecordingTitle key={recordingState.sessionId} sessionId={recordingState.sessionId} />
                </div>
              )}
              <LiveAssistanceDock
                sessionId={isRecording && !isStopping && !isProcessingStop && recordingState.sessionMode === 'live' ? recordingState.sessionId : null}
                verificationError={isRecordingOrStarting && !isStopping ? recordingState.verificationError : null}
                onRetryStatus={() => { recordingState.retryVerification(); void liveAssistance.refresh(); }}>
                <TranscriptPanel isProcessingStop={isProcessingStop} isStopping={isStopping} />
              </LiveAssistanceDock>
            </div>

            {/* Floating pause/stop controls for the active recording. Hidden once
                we move into transcript processing/saving (StatusOverlays take over). */}
            {(hasMicrophone || isRecording) && !recordingState.floatingControlsVisible &&
              status !== RecordingStatus.PROCESSING_TRANSCRIPTS &&
              status !== RecordingStatus.SAVING && (
                <div className="fixed bottom-12 left-0 right-0 z-10 pointer-events-none">
                  <div
                    className={`flex justify-center ${isSidebarResizing ? '' : 'transition-[margin] duration-300'}`}
                    style={{ marginLeft: sidebarOffset }}
                  >
                    <div className="pointer-events-auto rounded-full border border-border shadow-lg">
                      <RecordingControls
                        variant="floating"
                        isRecording={recordingState.isRecording}
                        onRecordingStart={startFromHome}
                        onStopInitiated={() => recordingState.setStatus(RecordingStatus.STOPPING)}
                        onTranscriptionError={(message) => showModal('errorAlert', message)}
                        isRecordingDisabled={isRecordingDisabled || (!isRecording && Boolean(recordingActionReason))}
                        startDisabledReason={recordingActionReason}
                        isParentProcessing={isProcessingStop}
                        selectedDevices={selectedDevices}
                        meetingName={meetingTitle}
                      />
                    </div>
                  </div>
                </div>
              )}

            <StatusOverlays
              isProcessing={status === RecordingStatus.PROCESSING_TRANSCRIPTS && !recordingState.isRecording}
              isSaving={status === RecordingStatus.SAVING}
            />
          </>
        ) : (
          <HomeDashboard
            isRecording={recordingState.isRecording}
            isProcessingStop={isProcessingStop}
            isRecordingDisabled={isRecordingDisabled}
            meetingName={meetingTitle}
            onRecordingStart={startFromHome}
            onStopInitiated={() => recordingState.setStatus(RecordingStatus.STOPPING)}
            onTranscriptionError={(message) => {
              showModal('errorAlert', message);
            }}
          />
        )}
      </div>
    </motion.div>
  );
}
