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
    let unlistenFn: (() => void) | undefined;

    const setupChunkDropListener = async () => {
      try {
        console.log('Setting up chunk-drop-warning listener...');
        unlistenFn = await listen<string>('chunk-drop-warning', (event) => {
          console.log('Chunk drop warning received:', event.payload);
          showModal('chunkDropWarning', event.payload);
        });
        console.log('Chunk drop warning listener setup complete');
      } catch (error) {
        console.error('Failed to setup chunk drop warning listener:', error);
      }
    };

    setupChunkDropListener();

    return () => {
      console.log('Cleaning up chunk drop warning listener...');
      if (unlistenFn) {
        unlistenFn();
      }
    };
  }, [showModal]);

  // Set up transcription error listener for model loading failures
  useEffect(() => {
    let unlistenFn: (() => void) | undefined;

    const setupTranscriptionErrorListener = async () => {
      try {
        console.log('Setting up transcription-error listener...');
        unlistenFn = await listen<{ error: string, userMessage: string, actionable: boolean }>('transcription-error', (event) => {
          console.log('Transcription error received:', event.payload);
          const { userMessage, actionable } = event.payload;

          if (actionable) {
            // This is a model-related error that requires user action
            showModal('modelSelector', userMessage);
          } else {
            // Show toast instead of modal for non-actionable errors (consistent with sidebar)
            toast.error('', {
              description: userMessage,
              duration: 5000,
            });
          }
        });
        console.log('Transcription error listener setup complete');
      } catch (error) {
        console.error('Failed to setup transcription error listener:', error);
      }
    };

    setupTranscriptionErrorListener();

    return () => {
      console.log('Cleaning up transcription error listener...');
      if (unlistenFn) {
        unlistenFn();
      }
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
