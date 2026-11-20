'use client'

import "sonner/dist/styles.css"
import './globals.css'
import {
  Atkinson_Hyperlegible,
  Fira_Sans,
  Geist,
  IBM_Plex_Mono,
  IBM_Plex_Sans,
  Inter,
  Lexend,
  Source_Sans_3,
  Source_Serif_4,
} from 'next/font/google'
import Sidebar from '@/components/Sidebar'
import { SidebarProvider } from '@/components/Sidebar/SidebarProvider'
import MainContent from '@/components/MainContent'
import { cleanupTranscriptRecoveryOnce } from '@/services/indexedDBService'
import { migrateLegacyMeetingContexts } from '@/lib/meetingContext'
import { flushMeetingDrafts, flushSummaryEditsBeforeExit } from '@/lib/meetingDrafts'
import { AppTitlebar } from '@/components/AppTitlebar'
import AnalyticsProvider from '@/components/AnalyticsProvider'
import { toast } from 'sonner'
import { AppToaster } from '@/components/AppToaster'
import { useState, useEffect, useCallback } from 'react'
import { listen, UnlistenFn } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import { TooltipProvider } from '@/components/ui/tooltip'
import { RecordingStateProvider } from '@/contexts/RecordingStateContext'
import { LiveAssistanceProvider } from '@/contexts/LiveAssistanceContext'
import { OllamaDownloadProvider } from '@/contexts/OllamaDownloadContext'
import { TranscriptProvider } from '@/contexts/TranscriptContext'
import { ConfigProvider } from '@/contexts/ConfigContext'
import { OnboardingProvider } from '@/contexts/OnboardingContext'
import { OnboardingFlow } from '@/components/onboarding'
import { DownloadProgressToastProvider } from '@/components/shared/DownloadProgressToast'
import { UpdateCheckProvider } from '@/components/UpdateCheckProvider'
import { RecordingPostProcessingProvider } from '@/contexts/RecordingPostProcessingProvider'
import { ImportAudioDialog, ImportDropOverlay } from '@/components/ImportAudio'
import { ImportDialogProvider } from '@/contexts/ImportDialogContext'
import { isAudioExtension, getAudioFormatsDisplayList } from '@/constants/audioFormats'
import { installTeamsDetectionDebugBridge } from '@/services/teamsDetectionService'
import { useRouter } from 'next/navigation'
import { ThemeInitializer } from '@/components/ThemeSettings'
import { AppShortcuts } from '@/components/AppShortcuts'
import { TeamsAutoRecord } from '@/components/TeamsAutoRecord'
import { AudioDeviceHotSwapMonitor } from '@/components/AudioDeviceHotSwapMonitor'


const sourceSans3 = Source_Sans_3({
  subsets: ['latin'],
  weight: ['400', '500', '600', '700'],
  variable: '--font-source-sans-3',
})

const atkinson = Atkinson_Hyperlegible({
  subsets: ['latin'],
  weight: ['400', '700'],
  variable: '--font-atkinson',
})

const lexend = Lexend({
  subsets: ['latin'],
  weight: ['400', '500', '600', '700'],
  variable: '--font-lexend',
})

const firaSans = Fira_Sans({
  subsets: ['latin'],
  weight: ['400', '500', '600', '700'],
  variable: '--font-fira-sans',
})

const ibmPlexSans = IBM_Plex_Sans({
  subsets: ['latin'],
  weight: ['400', '500', '600', '700'],
  variable: '--font-ibm-plex-sans',
})

const inter = Inter({
  subsets: ['latin'],
  variable: '--font-inter',
})

const geist = Geist({
  subsets: ['latin'],
  variable: '--font-geist',
})

const sourceSerif4 = Source_Serif_4({
  subsets: ['latin'],
  variable: '--font-source-serif-4',
})

// Utility/data face: timestamps, durations, speaker labels, model ids, paths,
// shortcuts. Transcription is data, so its values read as data.
const plexMono = IBM_Plex_Mono({
  subsets: ['latin'],
  weight: ['400', '500', '600'],
  variable: '--font-plex-mono',
})

// On <html>, so the font tokens set on the root element can resolve them.
const fontVariables = [sourceSans3, atkinson, lexend, firaSans, ibmPlexSans, inter, geist, sourceSerif4, plexMono]
  .map((font) => font.variable)
  .join(' ')

function AppImportDialog({
  showImportDialog,
  handleImportDialogClose,
  importFilePath,
}: {
  showImportDialog: boolean;
  handleImportDialogClose: (open: boolean) => void;
  importFilePath: string | null;
}) {
  return (
    <ImportAudioDialog
      open={showImportDialog}
      onOpenChange={handleImportDialogClose}
      preselectedFile={importFilePath}
    />
  );
}

export default function RootLayout({
  children,
}: {
  children: React.ReactNode
}) {
  const router = useRouter()
  const [showOnboarding, setShowOnboarding] = useState(false)
  const [onboardingCompleted, setOnboardingCompleted] = useState(false)

  useEffect(() => {
    const close = listen('summary-flush-on-close', () => { void flushMeetingDrafts().catch(() => {}); });
    const quit = listen<number>('summary-flush-before-exit', event => {
      void flushSummaryEditsBeforeExit(event.payload).catch(() => {
        toast.error('Could not notify ClawScribe about the save failure. A quit confirmation will appear shortly.');
      });
    });
    return () => { void close.then(unlisten => unlisten()); void quit.then(unlisten => unlisten()); };
  }, []);

  useEffect(() => {
    void migrateLegacyMeetingContexts().catch(() => toast.error('Could not migrate meeting context. Existing context is retained for retry.'));
  }, []);

  useEffect(() => {
    void cleanupTranscriptRecoveryOnce().catch(() => toast.warning('Could not clean saved transcript recovery copies.'));
  }, []);

  // Import audio state
  const [showDropOverlay, setShowDropOverlay] = useState(false)
  const [showImportDialog, setShowImportDialog] = useState(false)
  const [importFilePath, setImportFilePath] = useState<string | null>(null)

  useEffect(() => {
    installTeamsDetectionDebugBridge()
  }, [])

  useEffect(() => {
    // The core owns the behaviour settings (engine options, cloud opt-in,
    // source attribution, language) and has applied them before this window
    // existed. Read them here; the first time, the old webview values are
    // handed to the core, which keeps its own wherever it already has one.
    void import('@/lib/behaviorSettings')
      .then((m) => m.loadBehaviorSettings())
      .catch((error) => console.error('[Layout] Could not read the behaviour settings:', error))
  }, [])

  useEffect(() => {
    // Check onboarding status first
    invoke<{ completed: boolean } | null>('get_onboarding_status')
      .then((status) => {
        const isComplete = status?.completed ?? false
        setOnboardingCompleted(isComplete)

        if (!isComplete) {
          console.log('[Layout] Onboarding not completed, showing onboarding flow')
          setShowOnboarding(true)
        } else {
          console.log('[Layout] Onboarding completed, showing main app')
        }
      })
      .catch((error) => {
        console.error('[Layout] Failed to check onboarding status:', error)
        // Default to showing onboarding if we can't check
        setShowOnboarding(true)
        setOnboardingCompleted(false)
      })
  }, [])

  // Disable context menu in production
  useEffect(() => {
    if (process.env.NODE_ENV === 'production') {
      const handleContextMenu = (e: MouseEvent) => e.preventDefault();
      document.addEventListener('contextmenu', handleContextMenu);
      return () => document.removeEventListener('contextmenu', handleContextMenu);
    }
  }, []);
  useEffect(() => {
    // Listen for tray recording toggle request
    const unlisten = listen('request-recording-toggle', () => {
      console.log('[Layout] Received request-recording-toggle from tray');

      if (showOnboarding) {
        toast.error("Please complete setup first", {
          description: "You need to finish onboarding before you can start recording."
        });
      } else {
        // If in main app, forward to useRecordingStart via window event
        console.log('[Layout] Forwarding to start-recording-from-sidebar');
        window.dispatchEvent(new CustomEvent('start-recording-from-sidebar'));
      }
    });

    return () => {
      unlisten.then(fn => fn());
    };
  }, [showOnboarding]);

  // Surface non-fatal warnings raised by the recording pipeline.
  useEffect(() => {
    const unlisten = listen<string>('recording-warning', (event) => {
      const isTranscriptionWarning = event.payload.startsWith('Live transcription');
      toast.warning(isTranscriptionWarning ? 'Live transcription incomplete' : 'Recording warning', {
        description: event.payload,
        duration: 12000,
      });
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<{
      meeting_id?: string | null;
      provider?: string;
      reason_category?: string;
    }>('transcription-fell-back-to-local', (event) => {
      const reason = event.payload?.reason_category;
      if (reason === 'auth_config') {
        toast.warning('Cloud transcription failed', {
          description: 'Check your API key or endpoint. ClawScribe is switching to local transcription.',
          duration: 14000,
          action: {
            label: 'Open settings',
            onClick: () => {
              router.push('/settings?tab=transcription');
              window.dispatchEvent(new CustomEvent('open-settings-tab', { detail: 'transcription' }));
            },
          },
        });
        return;
      }

      if (reason === 'upload_too_large') {
        toast.warning('Recording too large for cloud transcription', {
          description: 'The selected cloud provider rejected this recording as too large or too long. ClawScribe is switching to local transcription.',
          duration: 14000,
        });
        return;
      }

      if (reason === 'unsupported_media') {
        toast.warning('Audio format not supported by cloud provider', {
          description: 'The selected cloud provider rejected this audio format. ClawScribe is switching to local transcription.',
          duration: 14000,
        });
        return;
      }

      if (reason === 'provider_output') {
        toast.warning('Cloud transcript could not be used', {
          description: 'ClawScribe is switching to local transcription.',
          duration: 14000,
        });
        return;
      }

      toast.warning('Cloud transcription unavailable', {
        description: 'ClawScribe is switching to local transcription. Try again later.',
        duration: 12000,
      });
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [router]);

  // Handle file drop for audio import
  const handleFileDrop = useCallback((audioPaths: string[], otherCount: number) => {
    // The first audio file the core granted
    const audioFile = audioPaths.find(p => {
      const ext = p.split('.').pop()?.toLowerCase();
      return !!ext && isAudioExtension(ext);
    });

    if (audioFile) {
      console.log('[Layout] Audio file dropped');
      setImportFilePath(audioFile);
      setShowImportDialog(true);
    } else if (audioPaths.length + otherCount > 0) {
      toast.error('Please drop an audio file', {
        description: `Supported formats: ${getAudioFormatsDisplayList()}`
      });
    }
  }, []);

  // Listen for drag-drop events
  useEffect(() => {
    if (showOnboarding) return; // Don't handle drops during onboarding

    const unlisteners: UnlistenFn[] = [];
    const cleanedUpRef = { current: false };

    const setupListeners = async () => {
      // Drag enter/over - show overlay
      const unlistenDragEnter = await listen('tauri://drag-enter', () => {
        setShowDropOverlay(true);
      });
      if (cleanedUpRef.current) {
        unlistenDragEnter();
        return;
      }
      unlisteners.push(unlistenDragEnter);

      // Drag leave - hide overlay
      const unlistenDragLeave = await listen('tauri://drag-leave', () => {
        setShowDropOverlay(false);
      });
      if (cleanedUpRef.current) {
        unlistenDragLeave();
        unlisteners.forEach(u => u());
        return;
      }
      unlisteners.push(unlistenDragLeave);

      // Drop - the core saw the drop and granted the audio files for import
      // before telling the window; the paths come from the core's event.
      const unlistenDrop = await listen<{ audio: string[]; other: number }>('files-dropped', (event) => {
        setShowDropOverlay(false);
        handleFileDrop(event.payload.audio ?? [], event.payload.other ?? 0);
      });
      if (cleanedUpRef.current) {
        unlistenDrop();
        unlisteners.forEach(u => u());
        return;
      }
      unlisteners.push(unlistenDrop);
    };

    setupListeners();

    return () => {
      cleanedUpRef.current = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [showOnboarding, handleFileDrop]);

  // Handle import dialog close
  const handleImportDialogClose = useCallback((open: boolean) => {
    setShowImportDialog(open);
    if (!open) {
      setImportFilePath(null);
    }
  }, []);

  // Handler for ImportDialogProvider - opens import dialog from any child component
  const handleOpenImportDialog = useCallback((filePath?: string | null) => {
    setImportFilePath(filePath ?? null);
    setShowImportDialog(true);
  }, []);

  const handleOnboardingComplete = () => {
    console.log('[Layout] Onboarding completed, reloading app')
    setShowOnboarding(false)
    setOnboardingCompleted(true)
    // Optionally reload the window to ensure all state is fresh
    window.location.reload()
  }

  return (
    <html lang="en" className={fontVariables}>
      <body className="font-sans antialiased">
        <ThemeInitializer />
        <AppShortcuts />
        <AppTitlebar />
        <AnalyticsProvider>
          <RecordingStateProvider>
            <AudioDeviceHotSwapMonitor />
            <TranscriptProvider>
              <ConfigProvider>
                <LiveAssistanceProvider>
                <OllamaDownloadProvider>
                  <OnboardingProvider>
                    <UpdateCheckProvider>
                      <SidebarProvider>
                        <TooltipProvider>
                          <RecordingPostProcessingProvider>
                            <ImportDialogProvider onOpen={handleOpenImportDialog}>
                              {/* Download progress toast provider - listens for background downloads */}
                              <DownloadProgressToastProvider />

                              {/* Auto-start recording when a Teams meeting is detected (opt-in) */}
                              <TeamsAutoRecord />

                              {/* Show onboarding or main app */}
                              {showOnboarding ? (
                                <div className="pt-[var(--titlebar-height)]">
                                  <OnboardingFlow onComplete={handleOnboardingComplete} />
                                </div>
                              ) : (
                                <div className="flex pt-[var(--titlebar-height)]">
                                  <Sidebar />
                                  <MainContent>{children}</MainContent>
                                </div>
                              )}
                              {/* Import audio overlay and dialog */}
                              <ImportDropOverlay visible={showDropOverlay} />
                              <AppImportDialog
                                showImportDialog={showImportDialog}
                                handleImportDialogClose={handleImportDialogClose}
                                importFilePath={importFilePath}
                              />
                            </ImportDialogProvider>
                          </RecordingPostProcessingProvider>
                        </TooltipProvider>
                      </SidebarProvider>
                    </UpdateCheckProvider>
                  </OnboardingProvider>

                </OllamaDownloadProvider>
                </LiveAssistanceProvider>
              </ConfigProvider>
            </TranscriptProvider>
          </RecordingStateProvider>
        </AnalyticsProvider>

        <AppToaster />
      </body>
    </html>
  )
}
