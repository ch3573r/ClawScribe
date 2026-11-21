'use client';

import React, { useRef, useState } from 'react';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { RecordingHealthBanner } from '@/components/RecordingHealthBanner';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { Button } from '@/components/ui/button';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';

interface MainContentProps {
  children: React.ReactNode;
  recoveryLibrary?: boolean;
}

function SavingRecordingBanner() {
  const { isStopping, sessionId, statusMessage } = useRecordingState();
  if (!isStopping) return null;
  const queueNext = async () => {
    const update = (enabled: boolean) => invoke<boolean>('queue_recording_after_save', { sessionId, enabled });
    if (!await update(true).catch(() => false)) {
      toast.info('Could not queue a new recording. Check the recording status and try again.');
      return;
    }
    toast.info('New recording queued for after this save', {
      id: 'recording-start-feedback',
      description: 'The request expires in one minute.',
      action: { label: 'Cancel', onClick: () => {
        void update(false).then(cancelled => toast.info(cancelled ? 'Queued recording canceled' : 'The save has already finished. Check the recording status.'))
          .catch(() => toast.error('Could not cancel the queued recording. Check the recording status.'));
      } },
    });
  };
  return (
    <div className="flex shrink-0 items-center justify-between gap-2 border-b border-border bg-muted px-4 py-2 text-sm">
      <span role="status">{statusMessage || 'Stopping recording…'}</span>
      {sessionId && statusMessage && (
        <Button size="sm" variant="outline" onClick={queueNext}>Start after saving</Button>
      )}
    </div>
  );
}

function RecoveryLibraryBanner() {
  const [returning, setReturning] = useState(false);
  const [selectedOriginal, setSelectedOriginal] = useState(false);
  const pending = useRef(false);
  const selectOriginal = async () => {
    if (pending.current) return;
    pending.current = true;
    setReturning(true);
    try {
      await invoke('return_to_original_library');
      setSelectedOriginal(true);
    } catch (error) {
      toast.error('Could not select the original library', { description: String(error) });
    } finally {
      pending.current = false;
      setReturning(false);
    }
  };
  return <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-b border-border bg-muted px-4 py-2 text-sm">
    <p role="status"><strong>Recovery library.</strong>{' '}
      {selectedOriginal ? 'Quit and reopen ClawScribe to use your original library.' : 'Restore a ZIP from Meetings, or select your original library for the next launch.'}
    </p>
    {!selectedOriginal && <Button size="sm" variant="outline" disabled={returning} onClick={selectOriginal}>
      {returning ? 'Selecting original library…' : 'Return to original library'}
    </Button>}
  </div>;
}

const MainContent: React.FC<MainContentProps> = ({ children, recoveryLibrary }) => {
  const { sidebarOffset, isSidebarResizing } = useSidebar();
  return (
    <main
      style={{ marginLeft: sidebarOffset }}
      className={`flex flex-col h-[calc(100vh-var(--titlebar-height))] min-h-0 flex-1 overflow-hidden ${isSidebarResizing ? '' : 'transition-all duration-300'}`}
    >
      <RecordingHealthBanner />
      <SavingRecordingBanner />
      {recoveryLibrary && <RecoveryLibraryBanner />}
      <div className="min-h-0 flex-1 overflow-hidden">{children}</div>
    </main>
  );
};

export default MainContent;
