'use client';

import { createContext, useContext, useCallback, useMemo, useRef, useState, ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useImportQueue } from '@/hooks/useImportAudio';

interface ImportDialogContextType {
  openImportDialog: (filePath?: string | null) => void;
}

const ImportDialogContext = createContext<ImportDialogContextType | null>(null);
interface ImportQueueContextType extends ReturnType<typeof useImportQueue> {
  detecting: string | null;
  detected: ReadonlySet<string>;
  detectSpeakers: (meetingId: string) => Promise<boolean>;
}

const ImportQueueContext = createContext<ImportQueueContextType | null>(null);

export const useImportDialog = () => {
  const ctx = useContext(ImportDialogContext);
  if (!ctx) throw new Error('useImportDialog must be used within ImportDialogProvider');
  return ctx;
};

export function useBackgroundImportQueue() {
  const queue = useContext(ImportQueueContext);
  if (!queue) throw new Error('useBackgroundImportQueue must be used within ImportDialogProvider');
  return queue;
}

interface ImportDialogProviderProps {
  children: ReactNode;
  onOpen: (filePath?: string | null) => void;
}

export function ImportDialogProvider({ children, onOpen }: ImportDialogProviderProps) {
  const openImportDialog = useCallback((filePath?: string | null) => {
    onOpen(filePath);
  }, [onOpen]);
  const dialog = useMemo(() => ({ openImportDialog }), [openImportDialog]);
  const { items, error, recoveryNotice, loading } = useImportQueue(true);
  const [detecting, setDetecting] = useState<string | null>(null);
  const [detected, setDetected] = useState<ReadonlySet<string>>(() => new Set());
  const detectionPending = useRef(false);
  const detectSpeakers = useCallback(async (meetingId: string) => {
    if (detectionPending.current) return false;
    detectionPending.current = true;
    setDetecting(meetingId);
    try {
      const { folder_path } = await invoke<{ folder_path?: string }>('api_get_meeting_metadata', { meetingId });
      if (!folder_path) throw new Error('The imported meeting folder is unavailable.');
      await invoke('start_speaker_diarization_command', {
        meetingId, meetingFolderPath: folder_path, numSpeakers: null, preserveExistingLabels: true,
      });
      setDetected(current => new Set(current).add(meetingId));
      return true;
    } finally {
      detectionPending.current = false;
      setDetecting(null);
    }
  }, []);
  const queue = useMemo(() => ({ items, error, recoveryNotice, loading, detecting, detected, detectSpeakers }),
    [items, error, recoveryNotice, loading, detecting, detected, detectSpeakers]);

  return (
    <ImportDialogContext.Provider value={dialog}>
      <ImportQueueContext.Provider value={queue}>
        {children}
      </ImportQueueContext.Provider>
    </ImportDialogContext.Provider>
  );
}
