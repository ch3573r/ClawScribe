"use client";

import { CustomVocabularySettings } from "@/components/CustomVocabularySettings";
import { TranscriptSettings } from "@/components/TranscriptSettings";
import { TranscriptionOptions } from "@/components/settings/TranscriptionOptions";
import { SettingsColumns, SettingsStack } from "@/components/ui/settings-layout";
import { useConfig } from "@/contexts/ConfigContext";

// The Transcription settings page: engine and models on top, options beside custom vocabulary below.
export function TranscriptionSettingsPage() {
  const { transcriptModelConfig, setTranscriptModelConfig } = useConfig();
  return (
    <SettingsStack wide>
      <TranscriptSettings transcriptModelConfig={transcriptModelConfig} setTranscriptModelConfig={setTranscriptModelConfig} pcTest />
      <SettingsColumns aside={<CustomVocabularySettings />}>
        <TranscriptionOptions />
      </SettingsColumns>
    </SettingsStack>
  );
}
