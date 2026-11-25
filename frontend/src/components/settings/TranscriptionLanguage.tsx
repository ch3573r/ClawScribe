"use client";

import { useEffect } from "react";
import { Globe } from "lucide-react";
import { toast } from "sonner";
import { ROW_SELECT } from "@/components/settings/settings-parts";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Meta, SettingsCard, SettingsRow } from "@/components/ui/settings-layout";
import { useTranscriptionConfig } from "@/contexts/ConfigContext";
import { useRecordingState } from "@/contexts/RecordingStateContext";
import {
  isAutoLanguage,
  languageOptions,
  languageSupport,
  storedLanguageLabel,
} from "@/lib/transcription-language";

const DESCRIPTIONS = {
  whisper: "The language spoken in your meetings. Auto detect follows the speakers",
  nemotron: "Nemotron does not detect the language. Choose the one spoken in your meetings, or use the system language",
  detects: "Parakeet detects the spoken language itself",
  hosted: "Live recording uses Parakeet, which detects the language itself. For an import or retranscription, choose the language in its dialog",
} as const;

/** The transcription language, shown only for engines that use it. */
export function TranscriptionLanguage() {
  const { transcriptModelConfig, selectedLanguage, setSelectedLanguage } = useTranscriptionConfig();
  const recording = useRecordingState();
  const busy = recording.isRecording || recording.isStarting || recording.isStopping;
  const support = languageSupport(transcriptModelConfig.provider);
  const options = languageOptions(support);

  // Nemotron transcribes but cannot translate. Auto already means the system
  // language for it, so only the translation choice needs to move.
  const cannotTranslate = support === "nemotron" && selectedLanguage === "auto-translate";
  useEffect(() => {
    if (!cannotTranslate || busy) return;
    setSelectedLanguage("auto");
    toast.info("Nemotron cannot translate", {
      description: "The transcription language was set to the system language. Choose another here if it is wrong.",
    });
  }, [cannotTranslate, busy, setSelectedLanguage]);

  // An engine that ignores the setting still reads a stored language for the
  // language saved with a recording and for the speaker-matching model. Show it
  // and let the user clear it; never change it silently.
  const staleValue = options.length === 0 && selectedLanguage !== "auto";

  return (
    <SettingsCard>
      <SettingsRow
        icon={<Globe aria-hidden="true" />}
        htmlFor={options.length ? "transcription-language" : undefined}
        title="Transcription language"
        description={<>
          {DESCRIPTIONS[support]}
          {staleValue && (
            <span role="status" className="mt-1 block">
              Not used for transcription with this engine. {storedLanguageLabel(selectedLanguage)} is still saved with new recordings and picks the speaker-matching model.
            </span>
          )}
          {options.length > 0 && busy && (
            <span role="status" className="mt-1 block">Not available while a recording is running.</span>
          )}
        </>}
      >
        {options.length ? (
          <Select
            value={options.some(option => option.code === selectedLanguage) ? selectedLanguage : ""}
            onValueChange={setSelectedLanguage}
            disabled={busy}
          >
            <SelectTrigger id="transcription-language" aria-label="Transcription language" className={ROW_SELECT}>
              <SelectValue placeholder="Choose a language" />
            </SelectTrigger>
            <SelectContent className="max-h-72">
              {options.map(option => (
                <SelectItem key={option.code} value={option.code}>
                  {option.label}
                  {!isAutoLanguage(option.code) && ` (${option.code})`}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        ) : staleValue ? (
          <>
            <Meta>{storedLanguageLabel(selectedLanguage)}</Meta>
            <Button type="button" variant="outline" size="sm" disabled={busy} onClick={() => setSelectedLanguage("auto")}>
              Reset to automatic
            </Button>
          </>
        ) : (
          <Meta>Automatic</Meta>
        )}
      </SettingsRow>
    </SettingsCard>
  );
}
