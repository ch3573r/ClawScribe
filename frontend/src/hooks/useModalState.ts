import { useState, useEffect, useCallback } from 'react';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { TranscriptModelProps } from '@/components/TranscriptSettings';
import type { LiveModelFailure } from '@/lib/start-error';

export type ModalType =
  | 'modelSelector'
  | 'errorAlert'
  | 'chunkDropWarning';

interface ModalState {
  modelSelector: boolean;
  errorAlert: boolean;
  chunkDropWarning: boolean;
}

interface ModalMessages {
  errorAlert: string;
  chunkDropWarning: string;
  modelSelector: string;
}

interface UseModalStateReturn {
  modals: ModalState;
  messages: ModalMessages;
  showModal: (name: ModalType, message?: string) => void;
  hideModal: (name: ModalType) => void;
  hideAllModals: () => void;
  modelStartFailure: LiveModelFailure | null;
  dismissModelStartFailure: () => void;
}

/**
 * Custom hook for managing all modal state and event listeners.
 * Consolidates 9 useState calls and 3 event listeners from page.tsx.
 *
 * Features:
 * - Unified modal state management
 * - Event listeners for chunk drops, transcription errors, model downloads
 * - Auto-close on model download completion
 */
export function useModalState(transcriptModelConfig?: TranscriptModelProps): UseModalStateReturn {
  const [modelStartFailure, setModelStartFailure] = useState<LiveModelFailure | null>(null);
  useEffect(() => {
    let active = true;
    const subscription = listen<LiveModelFailure>('live-model-start-failed', ({ payload }) => {
      if (active) setModelStartFailure(payload);
    });
    void subscription.catch(() => {});
    return () => {
      active = false;
      void subscription.then(unlisten => unlisten()).catch(() => {});
    };
  }, []);
  // Modal visibility state
  const [modals, setModals] = useState<ModalState>({
    modelSelector: false,
    errorAlert: false,
    chunkDropWarning: false,
  });

  // Modal messages
  const [messages, setMessages] = useState<ModalMessages>({
    errorAlert: '',
    chunkDropWarning: '',
    modelSelector: '',
  });

  // Show modal with optional message
  const showModal = useCallback((name: ModalType, message?: string) => {
    setModals(prev => ({ ...prev, [name]: true }));

    // Set message if provided
    if (message && (name === 'errorAlert' || name === 'chunkDropWarning' || name === 'modelSelector')) {
      setMessages(prev => ({ ...prev, [name]: message }));
    }
  }, []);

  // Hide modal and clear its message
  const hideModal = useCallback((name: ModalType) => {
    setModals(prev => ({ ...prev, [name]: false }));

    // Clear message when closing
    if (name === 'errorAlert' || name === 'chunkDropWarning' || name === 'modelSelector') {
      setMessages(prev => ({ ...prev, [name]: '' }));
    }
  }, []);

  // Hide all modals
  const hideAllModals = useCallback(() => {
    setModals({
      modelSelector: false,
      errorAlert: false,
      chunkDropWarning: false,
    });
    setMessages({
      errorAlert: '',
      chunkDropWarning: '',
      modelSelector: '',
    });
  }, []);

  // Set up chunk drop warning listener
  useEffect(() => {
    let active = true;
    const subscription = listen<string>('chunk-drop-warning', ({ payload }) => {
      if (active) showModal('chunkDropWarning', payload);
    });
    void subscription.catch(() => { if (active) console.error('Could not subscribe to chunk drop warnings'); });
    return () => {
      active = false;
      void subscription.then(unlisten => unlisten()).catch(() => {});
    };
  }, [showModal]);

  // Set up transcription error listener for model loading failures
  useEffect(() => {
    let active = true;
    const subscription = listen<{ userMessage: string, actionable: boolean }>('transcription-error', ({ payload }) => {
      if (!active) return;
      if (payload.actionable) showModal('modelSelector', payload.userMessage);
      else toast.error('', { description: payload.userMessage, duration: 5000 });
    });
    void subscription.catch(() => { if (active) console.error('Could not subscribe to transcription errors'); });
    return () => {
      active = false;
      void subscription.then(unlisten => unlisten()).catch(() => {});
    };
  }, [showModal]);

  // Listen for model download completion to auto-close modal
  useEffect(() => {
    let disposed = false;
    let closeTimer: ReturnType<typeof setTimeout> | undefined;
    const subscription = listen<{ modelName: string }>('model-download-complete', ({ payload }) => {
      if (disposed || transcriptModelConfig?.provider !== 'localWhisper' || transcriptModelConfig.model !== payload.modelName) return;
      toast.success('Model ready, closing this window', { duration: 1500 });
      clearTimeout(closeTimer);
      closeTimer = setTimeout(() => hideModal('modelSelector'), 1500);
    }).catch(() => undefined);

    return () => {
      disposed = true;
      clearTimeout(closeTimer);
      void subscription.then(unlisten => unlisten?.());
    };
  }, [transcriptModelConfig?.provider, transcriptModelConfig?.model, hideModal]);

  return {
    modals,
    messages,
    showModal,
    hideModal,
    hideAllModals,
    modelStartFailure,
    dismissModelStartFailure: () => setModelStartFailure(null),
  };
}
