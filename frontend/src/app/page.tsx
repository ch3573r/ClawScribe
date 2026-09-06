'use client';
import { useCompactLayout } from '@/hooks/useCompactLayout';

import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { appDataDir } from '@tauri-apps/api/path';
import { listen } from '@tauri-apps/api/event';
import { motion } from 'framer-motion';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { usePermissionCheck } from '@/hooks/usePermissionCheck';
import { useRecordingState, RecordingStatus } from '@/contexts/RecordingStateContext';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useConfig } from '@/contexts/ConfigContext';
import { StatusOverlays } from '@/app/_components/StatusOverlays';
import Analytics from '@/lib/analytics';
import { SettingsModals } from './_components/SettingsModal';
import { TranscriptPanel } from './_components/TranscriptPanel';
import { HomeDashboard } from '@/components/HomeDashboard';
import { RecordingControls } from '@/components/RecordingControls';
import { useModalState } from '@/hooks/useModalState';
import { useRecordingStateSync } from '@/hooks/useRecordingStateSync';
import { useRecordingStart } from '@/hooks/useRecordingStart';
import { useRecordingStop } from '@/hooks/useRecordingStop';
import { useTranscriptRecovery } from '@/hooks/useTranscriptRecovery';
import { TranscriptRecovery } from '@/components/TranscriptRecovery';
import { indexedDBService } from '@/services/indexedDBService';
import { toast } from 'sonner';
import { useRouter } from 'next/navigation';

export default function Home() {
  // Local page state (not moved to contexts)
  const [isRecording, setIsRecordingState] = useState(false);
  const [barHeights, setBarHeights] = useState(['58%', '76%', '58%']);
  const [showRecoveryDialog, setShowRecoveryDialog] = useState(false);

  // Use contexts for state management
  const { meetingTitle, transcripts } = useTranscripts();
  const { transcriptModelConfig, selectedDevices } = useConfig();
  const recordingState = useRecordingState();

  // Extract status from global state
  const { status, isStarting, isStopping, isProcessing, isSaving } = recordingState;

  // Hooks
  const { hasMicrophone } = usePermissionCheck();
  const { setIsMeetingActive, isCollapsed: sidebarCollapsed, refetchMeetings } = useSidebar();
  const { modals, messages, showModal, hideModal } = useModalState(transcriptModelConfig);
  const { isRecordingDisabled, setIsRecordingDisabled } = useRecordingStateSync(isRecording, setIsRecordingState, setIsMeetingActive);
  const { handleRecordingStart } = useRecordingStart(isRecording, setIsRecordingState);

  // Get handleRecordingStop function and setIsStopping (state comes from global context)
  const { handleRecordingStop, setIsStopping } = useRecordingStop(
    setIsRecordingState,
    setIsRecordingDisabled
  );

  // Recovery hook
  const {
    recoverableMeetings,
    isLoading: isLoadingRecovery,
    isRecovering,
    checkForRecoverableTranscripts,
    recoverMeeting,
    loadMeetingTranscripts,
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

        // 1. Clean up old meetings (7+ days)
        try {
          await indexedDBService.deleteOldMeetings(7);
        } catch (error) {
          console.warn('⚠️ Failed to clean up old meetings:', error);
        }

        // 2. Clean up saved meetings (24+ hours after save)
        try {
          await indexedDBService.deleteSavedMeetings(24);
        } catch (error) {
          console.warn('⚠️ Failed to clean up saved meetings:', error);
        }

        // 3. Always check for recoverable meetings on startup
        // Don't skip based on sessionStorage - we need to check every time
        await checkForRecoverableTranscripts();
      } catch (error) {
        console.error('Failed to perform startup checks:', error);
      }
    };

    performStartupChecks();
  }, [checkForRecoverableTranscripts, recordingState.isRecording, status]);

  // Watch for recoverable meetings changes and show dialog once per session
  useEffect(() => {
    // Only show dialog if we have meetings and haven't shown it yet this session
    if (recoverableMeetings.length > 0) {
      const shownThisSession = sessionStorage.getItem('recovery_dialog_shown');
      if (!shownThisSession) {
        setShowRecoveryDialog(true);
        sessionStorage.setItem('recovery_dialog_shown', 'true');
      }
    }
  }, [recoverableMeetings]);

  // Handle recovery with toast notifications and navigation
  const handleRecovery = async (meetingId: string) => {
    try {
      const result = await recoverMeeting(meetingId);

      if (result.success) {
        toast.success('Meeting recovered successfully!', {
          description: result.audioRecoveryStatus?.status === 'success'
            ? 'Transcripts and audio recovered'
            : 'Transcripts recovered (no audio available)',
          action: result.meetingId ? {
            label: 'View Meeting',
            onClick: () => {
              router.push(`/meeting-details?id=${result.meetingId}`);
            }
          } : undefined,
          duration: 10000,
        });

        // Refresh sidebar to show the newly recovered meeting
        await refetchMeetings();

        // If no more recoverable meetings, clear session flag so dialog can show again
        if (recoverableMeetings.length === 0) {
          sessionStorage.removeItem('recovery_dialog_shown');
        }

        // Auto-navigate after a short delay
        if (result.meetingId) {
          setTimeout(() => {
            router.push(`/meeting-details?id=${result.meetingId}`);
          }, 2000);
        }
      }
    } catch (error) {
      toast.error('Failed to recover meeting', {
        description: error instanceof Error ? error.message : 'Unknown error occurred',
      });
      throw error;
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

  // Stop the recording when the sidebar "Recording…" indicator is clicked.
  // Mirrors RecordingControls.stopRecordingAction so the stop flow is identical
  // regardless of which control triggered it.
  useEffect(() => {
    const onStopFromSidebar = async () => {
      if (!recordingState.isRecording || isStopping) return;
      setIsStopping(true);
      try {
        const dataDir = await appDataDir();
        const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
        const savePath = `${dataDir}/recording-${timestamp}.wav`;
        await invoke('stop_recording', { args: { save_path: savePath } });
        await handleRecordingStop(true);
      } catch (error) {
        console.error('Failed to stop recording from sidebar:', error);
        await handleRecordingStop(false);
      }
    };
    window.addEventListener('stop-recording-from-sidebar', onStopFromSidebar);
    return () => window.removeEventListener('stop-recording-from-sidebar', onStopFromSidebar);
  }, [recordingState.isRecording, isStopping, handleRecordingStop, setIsStopping]);

  const compact = useCompactLayout();
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

      {/* Recovery Dialog */}
      <TranscriptRecovery
        isOpen={showRecoveryDialog}
        onClose={handleDialogClose}
        recoverableMeetings={recoverableMeetings}
        onRecover={handleRecovery}
        onDelete={deleteRecoverableMeeting}
        onLoadPreview={loadMeetingTranscripts}
      />
      <div className="flex flex-1 overflow-hidden">
        {isRecordingOrStarting || transcripts.length > 0 || isProcessingStop || isStopping ? (
          <>
            <TranscriptPanel
              isProcessingStop={isProcessingStop}
              isStopping={isStopping}
              showModal={showModal}
            />

            {/* Floating pause/stop controls for the active recording. Hidden once
                we move into transcript processing/saving (StatusOverlays take over). */}
            {(hasMicrophone || isRecording) &&
              status !== RecordingStatus.PROCESSING_TRANSCRIPTS &&
              status !== RecordingStatus.SAVING && (
                <div className="fixed bottom-12 left-0 right-0 z-10 pointer-events-none">
                  <div
                    className="flex justify-center pl-8 transition-[margin] duration-300"
                    style={{ marginLeft: compact || sidebarCollapsed ? '4rem' : '17.5rem' }}
                  >
                    <div className="pointer-events-auto rounded-full border border-border shadow-lg">
                      <RecordingControls
                        variant="floating"
                        isRecording={recordingState.isRecording}
                        onRecordingStop={(callApi = true) => handleRecordingStop(callApi)}
                        onRecordingStart={handleRecordingStart}
                        onTranscriptReceived={() => {}}
                        onStopInitiated={() => setIsStopping(true)}
                        barHeights={barHeights}
                        onTranscriptionError={(message) => showModal('errorAlert', message)}
                        isRecordingDisabled={isRecordingDisabled}
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
              sidebarCollapsed={sidebarCollapsed}
            />
          </>
        ) : (
          <HomeDashboard
            canRecord={hasMicrophone || isRecording}
            isRecording={recordingState.isRecording}
            isProcessingStop={isProcessingStop}
            isRecordingDisabled={isRecordingDisabled}
            barHeights={barHeights}
            meetingName={meetingTitle}
            onRecordingStart={handleRecordingStart}
            onRecordingStop={(callApi = true) => handleRecordingStop(callApi)}
            onStopInitiated={() => setIsStopping(true)}
            onTranscriptionError={(message) => {
              showModal('errorAlert', message);
            }}
          />
        )}
      </div>
    </motion.div>
  );
}
