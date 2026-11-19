"use client";

import { useEffect, useState } from "react";

/**
 * Speed measured by "Test this PC", remembered per model id so the model table
 * can compare models. Only values returned by `run_transcription_pc_test` are
 * stored; nothing here is estimated.
 */

export interface PcTestRecord {
  /** Seconds of processing per second of audio, as returned by the test. */
  realtimeFactor: number;
  /** Milliseconds since the epoch when the test finished. */
  measuredAt: number;
  /** Backend the model ran on, for example CPU or DirectML. */
  backend: string;
  /** True when a built-in sample produced no recognized speech. */
  missedSpeech: boolean;
}

export interface PcTestMeasurement {
  backend: string;
  realtimeFactor: number;
  samples: { recognized: boolean }[];
}

export const PC_TEST_STORAGE_KEY = "clawscribe.transcriptionPcTest.v1";
const CHANGE_EVENT = "clawscribe:pc-test-change";

function isRecord(value: unknown): value is PcTestRecord {
  if (!value || typeof value !== "object") return false;
  const record = value as Partial<PcTestRecord>;
  return typeof record.realtimeFactor === "number" && Number.isFinite(record.realtimeFactor) && record.realtimeFactor > 0
    && typeof record.measuredAt === "number" && Number.isFinite(record.measuredAt);
}

/** All stored results keyed by model id. Corrupt storage reads as empty. */
export function readPcTestResults(): Record<string, PcTestRecord> {
  if (typeof window === "undefined") return {};
  try {
    const parsed: unknown = JSON.parse(window.localStorage.getItem(PC_TEST_STORAGE_KEY) ?? "{}");
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const results: Record<string, PcTestRecord> = {};
    for (const [modelId, value] of Object.entries(parsed)) {
      if (isRecord(value)) {
        results[modelId] = {
          realtimeFactor: value.realtimeFactor,
          measuredAt: value.measuredAt,
          backend: typeof value.backend === "string" ? value.backend : "",
          missedSpeech: value.missedSpeech === true,
        };
      }
    }
    return results;
  } catch {
    return {};
  }
}

/** Remember one finished test. Returns false when it could not be stored or is not a usable measurement. */
export function savePcTestResult(modelId: string, measurement: PcTestMeasurement, now = Date.now()): boolean {
  if (typeof window === "undefined" || !modelId) return false;
  if (!Number.isFinite(measurement.realtimeFactor) || measurement.realtimeFactor <= 0) return false;
  try {
    const next = {
      ...readPcTestResults(),
      [modelId]: {
        realtimeFactor: measurement.realtimeFactor,
        measuredAt: now,
        backend: measurement.backend,
        missedSpeech: measurement.samples.some(sample => !sample.recognized),
      },
    };
    window.localStorage.setItem(PC_TEST_STORAGE_KEY, JSON.stringify(next));
  } catch {
    return false;
  }
  window.dispatchEvent(new CustomEvent(CHANGE_EVENT));
  return true;
}

/** Real-time multiple: 22 means one minute of audio takes about 2.7 seconds. */
export function speedOf(record: PcTestRecord): number {
  return 1 / record.realtimeFactor;
}

export function formatSpeed(speed: number): string {
  return `${speed >= 10 ? Math.round(speed) : Math.round(speed * 10) / 10}× real time`;
}

/** Bar width as a fraction of the fastest measured model, never below a visible sliver. */
export function speedFraction(speed: number, fastest: number): number {
  if (!(fastest > 0) || !(speed > 0)) return 0;
  return Math.max(0.04, Math.min(1, speed / fastest));
}

export function fastestSpeed(records: PcTestRecord[]): number {
  return records.reduce((best, record) => Math.max(best, speedOf(record)), 0);
}

const pad = (value: number) => String(value).padStart(2, "0");

/** "today 10:31", "yesterday 10:31", otherwise "19 Nov 10:31". */
export function formatMeasuredAt(timestamp: number, now = Date.now()): string {
  const at = new Date(timestamp);
  const current = new Date(now);
  const time = `${pad(at.getHours())}:${pad(at.getMinutes())}`;
  const day = (date: Date) => new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
  const days = Math.round((day(current) - day(at)) / 86_400_000);
  if (days === 0) return `today ${time}`;
  if (days === 1) return `yesterday ${time}`;
  const month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][at.getMonth()];
  return `${at.getDate()} ${month} ${time}`;
}

/** Stored results, kept current when a test finishes anywhere in the app. */
export function usePcTestResults(): Record<string, PcTestRecord> {
  const [results, setResults] = useState<Record<string, PcTestRecord>>({});
  useEffect(() => {
    const refresh = () => setResults(readPcTestResults());
    refresh();
    window.addEventListener(CHANGE_EVENT, refresh);
    window.addEventListener("storage", refresh);
    return () => {
      window.removeEventListener(CHANGE_EVENT, refresh);
      window.removeEventListener("storage", refresh);
    };
  }, []);
  return results;
}
