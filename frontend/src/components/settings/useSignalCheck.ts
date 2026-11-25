"use client";

import { useEffect, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { levelToDb } from "@/components/AudioLevelMeter";
import { holdPeak } from "./useAudioLevelFeed";

type Source = "microphone" | "systemAudio";
type SignalResult = "detected" | "noSignal" | "unavailable";
interface NativeReading {
  source: Source;
  rms: number;
  peak: number;
}
interface Reading {
  db: number;
  peakDb: number;
}

export function useSignalCheck(
  enabled: boolean,
  microphone: string | null,
  systemAudio: string | null,
) {
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<Record<Source, SignalResult> | null>(null);
  const [levels, setLevels] = useState<Partial<Record<Source, Reading>>>({});
  const [error, setError] = useState<string | null>(null);
  const pending = useRef(false);
  const current = useRef<object | null>(null);
  const configuration = useRef({ enabled, microphone, systemAudio });
  configuration.current = { enabled, microphone, systemAudio };

  useEffect(() => {
    setLevels({});
    setResult(null);
    setError(null);
    return () => { current.current = null; };
  }, [enabled, microphone, systemAudio]);

  const start = async () => {
    if (!enabled || pending.current) return;
    const session = {};
    current.current = session;
    pending.current = true;
    setLevels({});
    setResult(null);
    setError(null);
    setRunning(true);
    const isCurrent = () => current.current === session && configuration.current.enabled &&
      configuration.current.microphone === microphone && configuration.current.systemAudio === systemAudio;
    try {
      const updates = new Channel<NativeReading>();
      updates.onmessage = ({ source, rms, peak }) => {
        if (!isCurrent()) return;
        const db = levelToDb(rms);
        setLevels(previous => ({
          ...previous,
          [source]: { db, peakDb: holdPeak(previous[source]?.peakDb ?? null, Math.max(db, levelToDb(peak))) },
        }));
      };
      const outcome = await invoke<Record<Source, SignalResult>>("check_audio_signal", { microphone, systemAudio, updates });
      if (isCurrent()) setResult(outcome);
    } catch (cause) {
      if (isCurrent()) setError(typeof cause === "string" ? cause : "Could not check audio. Try again.");
    } finally {
      current.current = null;
      pending.current = false;
      setRunning(false);
    }
  };

  return { levels, result, error, running, start };
}
