import { useRef } from "react";
import { Button } from "@/components/ui/button";
import { TranscriptSettings } from "@/components/TranscriptSettings";
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { useTranscriptionConfig } from "@/contexts/ConfigContext";

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
  const { transcriptModelConfig, setTranscriptModelConfig } = useTranscriptionConfig();

  const previousFocus = useRef<HTMLElement | null>(null);
  const activeModal = modals.chunkDropWarning ? "chunkDropWarning"
    : modals.errorAlert ? "errorAlert"
      : modals.modelSelector ? "modelSelector" : null;
  const choosingModel = activeModal === "modelSelector";
  const setupRequired = choosingModel && Boolean(messages.modelSelector);

  const title = choosingModel
    ? setupRequired ? "Speech recognition setup required" : "Transcription model settings"
    : activeModal === "errorAlert" ? "Recording stopped" : "Transcription performance warning";

  return (
    <Dialog open={activeModal !== null} onOpenChange={open => {
      if (!open && activeModal) onClose(activeModal);
    }}>
      <DialogContent
        className={choosingModel ? "flex max-h-[90vh] w-[calc(100%-2rem)] max-w-4xl flex-col gap-0 bg-card p-0" : "w-[calc(100%-2rem)] bg-card"}
        onOpenAutoFocus={() => {
          previousFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        }}
        onCloseAutoFocus={event => {
          event.preventDefault();
          if (previousFocus.current?.isConnected) previousFocus.current.focus();
          previousFocus.current = null;
        }}
      >
        <DialogHeader className={choosingModel ? "border-b border-border px-6 pb-4 pt-6 pr-12" : "pr-6"}>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>
            {(activeModal && messages[activeModal]) || (choosingModel ? "Choose a model for speech recognition." : "")}
          </DialogDescription>
        </DialogHeader>
        {choosingModel && (
          <div className="min-h-0 overflow-y-auto px-6 py-4">
            <TranscriptSettings
              transcriptModelConfig={transcriptModelConfig}
              setTranscriptModelConfig={setTranscriptModelConfig}
              onModelSelect={() => onClose("modelSelector")}
            />
          </div>
        )}
        <DialogFooter className={choosingModel ? "border-t border-border px-6 py-4" : undefined}>
          <DialogClose asChild>
            <Button variant="outline">{choosingModel ? setupRequired ? "Cancel" : "Done" : "Dismiss"}</Button>
          </DialogClose>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
