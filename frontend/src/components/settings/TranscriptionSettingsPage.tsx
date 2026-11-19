"use client";

import { CustomVocabularySettings } from "@/components/CustomVocabularySettings";
import { TranscriptSettings } from "@/components/TranscriptSettings";
import { TranscriptionPcTest } from "@/components/TranscriptionPcTest";
import { useConfig } from "@/contexts/ConfigContext";

// The Transcription settings page. Starts as the previous composition.
export function TranscriptionSettingsPage() {
  const { transcriptModelConfig, setTranscriptModelConfig } = useConfig();
  return (
    <div className="space-y-6">
      <TranscriptSettings transcriptModelConfig={transcriptModelConfig} setTranscriptModelConfig={setTranscriptModelConfig} />
      <TranscriptionPcTest
        key={JSON.stringify([transcriptModelConfig.provider, transcriptModelConfig.model])}
        provider={transcriptModelConfig.provider}
        model={transcriptModelConfig.model}
      />
      <CustomVocabularySettings />
    </div>
  );
}
