'use client';

import React from 'react';
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

const MainContent: React.FC<MainContentProps> = ({ children, recoveryLibrary }) => {
  const { sidebarOffset, isSidebarResizing } = useSidebar();
  return (
    <main
      style={{ marginLeft: sidebarOffset }}
      className={`flex flex-col h-[calc(100vh-var(--titlebar-height))] min-h-0 flex-1 overflow-hidden ${isSidebarResizing ? '' : 'transition-all duration-300'}`}
    >
      <RecordingHealthBanner />
      <SavingRecordingBanner />
      {recoveryLibrary && <p className="shrink-0 border-b border-border bg-muted px-4 py-2 text-sm">
        <strong>Recovery library.</strong> Your original library is still on disk. Restore a ZIP from Meetings.
      </p>}
      <div className="min-h-0 flex-1 overflow-hidden">{children}</div>
    </main>
  );
};

export default MainContent;
