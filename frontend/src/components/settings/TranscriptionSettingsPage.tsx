"use client";

import { CustomVocabularySettings } from "@/components/CustomVocabularySettings";
import { TranscriptSettings } from "@/components/TranscriptSettings";
import { TranscriptionLanguage } from "@/components/settings/TranscriptionLanguage";
import { TranscriptionOptions } from "@/components/settings/TranscriptionOptions";
import { Button } from "@/components/ui/button";
import { SettingsCard, SettingsColumns, SettingsRow, SettingsStack } from "@/components/ui/settings-layout";
import { useConfig } from "@/contexts/ConfigContext";

// The Transcription settings page: engine and models on top, language and options beside custom vocabulary below.
export function TranscriptionSettingsPage() {
  const { transcriptModelConfig, setTranscriptModelConfig, transcriptConfigStatus, loadTranscriptConfig } = useConfig();
  return (
    <SettingsStack wide>
      {transcriptConfigStatus === "ready" ? (
        <TranscriptSettings transcriptModelConfig={transcriptModelConfig} setTranscriptModelConfig={setTranscriptModelConfig} pcTest />
      ) : (
        <SettingsCard>
          {transcriptConfigStatus === "loading"
            ? <SettingsRow title="Loading transcription settings…" role="status" />
            : <SettingsRow title="Could not read the selected model" role="alert">
                <Button variant="outline" size="sm" onClick={() => void loadTranscriptConfig()}>Try again</Button>
              </SettingsRow>}
        </SettingsCard>
      )}
      <SettingsColumns aside={<CustomVocabularySettings />}>
        <TranscriptionLanguage />
        <TranscriptionOptions />
      </SettingsColumns>
    </SettingsStack>
  );
}
