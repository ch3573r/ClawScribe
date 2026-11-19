"use client";

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { LEVEL_FLOOR_DB, levelToDb } from "@/components/AudioLevelMeter";

/**
 * Live microphone level for the Recording page.
 *
 * The native monitor (`start_audio_level_monitoring`) opens real input streams
 * and emits `audio-levels` about ten times a second. It only measures input
 * devices: there is no system audio (loopback) level in this feed.
 */

/** Device used when no microphone is chosen. The native monitor falls back to the Windows default input for an unknown name. */
export const DEFAULT_MICROPHONE_REQUEST = "default";

interface NativeLevel {
  device_name: string;
  rms_level: number;
  peak_level: number;
  is_active: boolean;
}

interface NativeLevelUpdate {
  levels: NativeLevel[];
}

interface MonitoredMicrophone {
  requested_name: string;
  device_name: string;
  used_default: boolean;
}

export type LevelFeedStatus = "idle" | "starting" | "live" | "error";

export interface MicrophoneLevel {
  deviceName: string;
  db: number;
  peakDb: number;
  active: boolean;
}

export interface AudioLevelFeed {
  status: LevelFeedStatus;
  error: string | null;
  /** Latest reading. `null` until the first update arrives. */
  microphone: MicrophoneLevel | null;
  /** The device that is really measured, and whether it replaced an unavailable preferred one. */
  monitored: { deviceName: string; usedDefault: boolean } | null;
  /** Starts the monitor again after an error. */
  retry: () => void;
}

/** Peak marker falls this many dB per update (about 30 dB a second). */
export const PEAK_FALL_DB = 3;

export function holdPeak(previousPeakDb: number | null, db: number): number {
  if (previousPeakDb === null) return db;
  return Math.max(db, previousPeakDb - PEAK_FALL_DB, LEVEL_FLOOR_DB);
}

/** "-25 dB", or "< -60 dB" at the floor. */
export function formatDb(db: number | null): string {
  if (db === null) return "";
  return db <= LEVEL_FLOOR_DB ? `< ${LEVEL_FLOOR_DB} dB` : `${Math.round(db)} dB`;
}

export function monitorRequestFor(micDevice: string | null): string {
  const name = micDevice ? micDevice.replace(/\s+\(input\)$/i, "").trim() : "";
  return name || DEFAULT_MICROPHONE_REQUEST;
}

// Start and stop commands share one native monitor. Running them in order keeps
// a late stop from an old effect from cutting off the monitor a new one started.
let queue: Promise<unknown> = Promise.resolve();
function enqueue<T>(task: () => Promise<T>): Promise<T> {
  const next = queue.then(task, task);
  queue = next.catch(() => undefined);
  return next;
}

/**
 * Starts monitoring while `enabled` and stops it when disabled, when the
 * microphone changes, and when the component leaves the screen.
 */
export function useAudioLevelFeed({ enabled, micDevice }: { enabled: boolean; micDevice: string | null }): AudioLevelFeed {
  const [status, setStatus] = useState<LevelFeedStatus>("idle");
  const [error, setError] = useState<string | null>(null);
  const [microphone, setMicrophone] = useState<MicrophoneLevel | null>(null);
  const [monitored, setMonitored] = useState<AudioLevelFeed["monitored"]>(null);
  const request = monitorRequestFor(micDevice);
  const [attempt, setAttempt] = useState(0);
  const peak = useRef<number | null>(null);

  useEffect(() => {
    if (!enabled) {
      setStatus("idle");
      setError(null);
      setMicrophone(null);
      setMonitored(null);
      return;
    }
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    let measured: string | null = null;
    peak.current = null;
    setStatus("starting");
    setError(null);
    setMicrophone(null);
    setMonitored(null);

    listen<NativeLevelUpdate>("audio-levels", event => {
      if (cancelled || measured === null) return;
      const level = event.payload.levels.find(item => item.device_name === measured);
      if (!level) return;
      const db = levelToDb(level.rms_level);
      peak.current = holdPeak(peak.current, Math.max(db, levelToDb(level.peak_level)));
      setMicrophone({ deviceName: level.device_name, db, peakDb: peak.current, active: level.is_active });
    }).then(stop => {
      if (cancelled) stop();
      else unlisten = stop;
    }).catch(problem => console.error("Failed to listen for audio levels:", problem));

    enqueue(() => invoke<MonitoredMicrophone[]>("start_audio_level_monitoring", { deviceNames: [request] }))
      .then(devices => {
        if (cancelled) return;
        const first = devices[0];
        if (!first) {
          setStatus("error");
          setError("No microphone could be measured.");
          return;
        }
        measured = first.device_name;
        setMonitored({ deviceName: first.device_name, usedDefault: first.used_default });
        setStatus("live");
      })
      .catch(problem => {
        if (cancelled) return;
        setStatus("error");
        setError(String(problem));
      });

    return () => {
      cancelled = true;
      unlisten?.();
      // Always queued behind the start above, so the microphone is released
      // even when the page is left while the start is still pending.
      void enqueue(() => invoke("stop_audio_level_monitoring")).catch(problem => console.error("Failed to stop audio level monitoring:", problem));
    };
  }, [enabled, request, attempt]);

  return { status, error, microphone, monitored, retry: () => setAttempt(value => value + 1) };
}
