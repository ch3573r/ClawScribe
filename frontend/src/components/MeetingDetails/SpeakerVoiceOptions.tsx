"use client";

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import type { SpeakerVoiceOption } from "@/lib/speaker-labels";

const EMPTY_OPTIONS: readonly SpeakerVoiceOption[] = [];
const VoiceOptions = createContext(EMPTY_OPTIONS);

export function SpeakerVoiceOptionsProvider({ meetingId, children }: { meetingId: string; children: ReactNode }) {
  const [result, setResult] = useState<{ meetingId: string; options: readonly SpeakerVoiceOption[] }>();

  useEffect(() => {
    let active = true;
    let version = 0;
    const load = async () => {
      const request = ++version;
      const options = await invoke<SpeakerVoiceOption[]>("get_meeting_speaker_voice_options", { meetingId }).catch(() => null);
      if (active && request === version) setResult({ meetingId, options: Array.isArray(options) ? options : EMPTY_OPTIONS });
    };
    const subscriptions = [
      listen("speaker-profiles-changed", () => void load()),
      listen("speaker-diarization-complete", () => void load()),
      ...["transcript-speaker-updated", "transcript-text-edited", "retranscription-complete"]
        .map(event => listen<{ meeting_id: string }>(event, ({ payload }) => {
          if (payload.meeting_id === meetingId) void load();
        })),
    ];
    const ready = Promise.allSettled(subscriptions);
    void ready.then(() => { if (active) void load(); });
    return () => {
      active = false;
      void ready.then(results => {
        for (const result of results) if (result.status === "fulfilled") result.value();
      });
    };
  }, [meetingId]);

  return <VoiceOptions.Provider value={result?.meetingId === meetingId ? result.options : EMPTY_OPTIONS}>
    {children}
  </VoiceOptions.Provider>;
}

export function useSpeakerVoiceOption(speaker: string) {
  return useContext(VoiceOptions).find(option => option.speaker === speaker);
}
