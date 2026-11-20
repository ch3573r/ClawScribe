import { Button } from "@/components/ui/button";
import { TranscriptSettings } from "@/components/TranscriptSettings";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { useConfig } from "@/contexts/ConfigContext";

type modalType = "modelSelector" | "errorAlert" | "chunkDropWarning";

/**
 * The dialogs the Home screen raises itself: speech recognition setup after an
 * actionable transcription error, the recording-stopped alert and the chunk drop
 * warning. Language, devices and summary models are set in Settings.
 */

interface SettingsModalsProps {
  modals: {
    modelSelector: boolean;
    errorAlert: boolean;
    chunkDropWarning: boolean;
  };
  messages: {
    errorAlert: string;
    chunkDropWarning: string;
    modelSelector: string;
  };
  onClose: (name: modalType) => void;
}

export function SettingsModals({
  modals,
  messages,
  onClose,
}: SettingsModalsProps) {
  const { transcriptModelConfig, setTranscriptModelConfig } = useConfig();

  return <>
    {/* Model Selection Modal */}
    {modals.modelSelector && (
      <div className="fixed inset-0 bg-overlay/50 flex items-center justify-center z-50">
        <div className="bg-card rounded-lg max-w-4xl w-full mx-4 shadow-xl max-h-[90vh] flex flex-col">
          {/* Fixed Header */}
          <div className="flex justify-between items-center p-6 pb-4 border-b border-border">
            <h3 className="text-lg font-semibold text-foreground">
              {messages.modelSelector ? 'Speech recognition setup required' : 'Transcription model settings'}
            </h3>
            <Button
              variant="ghost"
              aria-label="Close transcription model settings"
              onClick={() => onClose('modelSelector')}
              className="h-auto whitespace-normal p-0 font-normal hover:bg-transparent text-muted-foreground hover:text-foreground transition-colors"
            >
              <svg xmlns="http://www.w3.org/2000/svg" className="!h-6 !w-6" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
              </svg>
            </Button>
          </div>

          {/* Scrollable Content */}
          <div className="flex-1 overflow-y-auto p-6 pt-4">
            <TranscriptSettings
              transcriptModelConfig={transcriptModelConfig}
              setTranscriptModelConfig={setTranscriptModelConfig}
              onModelSelect={() => onClose('modelSelector')}
            />
          </div>

          {/* Fixed Footer */}
          <div className="p-6 pt-4 border-t border-border flex items-center justify-end">
            <Button
              variant="ghost"
              onClick={() => onClose('modelSelector')}
              className="h-auto whitespace-normal px-4 py-2 text-sm font-medium text-foreground bg-secondary rounded-md hover:bg-secondary/80 focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-ring transition-colors hover:text-foreground"
            >
              {messages.modelSelector ? 'Cancel' : 'Done'}
            </Button>
          </div>
        </div>
      </div>
    )}

    {/* Error Alert Modal */}
    {modals.errorAlert && (
      <div className="fixed inset-0 bg-overlay/50 flex items-center justify-center z-50">
        <Alert className="max-w-md mx-4 border-error-border/30 bg-card shadow-xl">
          <AlertTitle className="text-error-foreground">Recording stopped</AlertTitle>
          <AlertDescription className="text-error-foreground">
            {messages.errorAlert}
            <Button
              variant="ghost"
              onClick={() => onClose('errorAlert')}
              className="h-auto whitespace-normal p-0 font-normal hover:bg-transparent ml-2 text-error-foreground hover:text-error-foreground underline"
            >
              Dismiss
            </Button>
          </AlertDescription>
        </Alert>
      </div>
    )}

    {/* Chunk Drop Warning Modal */}
    {modals.chunkDropWarning && (
      <div className="fixed inset-0 bg-overlay/50 flex items-center justify-center z-50">
        <Alert className="max-w-lg mx-4 border-warning-border/30 bg-card shadow-xl">
          <AlertTitle className="text-warning-foreground">Transcription performance warning</AlertTitle>
          <AlertDescription className="text-warning-foreground">
            {messages.chunkDropWarning}
            <Button
              variant="ghost"
              onClick={() => onClose('chunkDropWarning')}
              className="h-auto whitespace-normal p-0 font-normal hover:bg-transparent ml-2 text-warning-foreground hover:text-warning-foreground underline"
            >
              Dismiss
            </Button>
          </AlertDescription>
        </Alert>
      </div>
    )}
  </>
}
