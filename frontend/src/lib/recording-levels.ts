import { LEVEL_FLOOR_DB, levelToDb } from "@/components/AudioLevelMeter";
import { holdPeak } from "@/components/settings/useAudioLevelFeed";

/**
 * Levels of a running recording, from the core's `recording-levels` event about
 * ten times a second. Linear 0..1; a source that is not captured is `null`.
 * Nothing is sent while paused or stopped.
 */
export interface SourceLevelPayload { rms: number; peak: number }
export interface RecordingLevelsPayload { mic: SourceLevelPayload | null; system: SourceLevelPayload | null }

/** No event for this long while recording reads as silence. */
export const SILENCE_AFTER_MS = 500;
/** A microphone that has not made a sound this long since the start is worth a quiet note. */
export const MIC_NO_SIGNAL_AFTER_MS = 10_000;
/** Linear peak below which a source counts as silent (the meter floor, -60 dB). */
const SILENT_PEAK = 0.001;

export type SourceState =
  /** Nothing received yet. */
  | { kind: "unknown" }
  /** The core does not capture this source. */
  | { kind: "absent" }
  | { kind: "level"; db: number; peakDb: number; silentSince: number | null };

export interface MeterState {
  mic: SourceState;
  system: SourceState;
  /** When the last event arrived, or null. */
  lastEventAt: number | null;
  /** The microphone has been above the floor at least once since the recording started or resumed. */
  micSeenSignal: boolean;
  /**
   * The microphone has made no sound at all for MIC_NO_SIGNAL_AFTER_MS since the
   * recording started or resumed: a dead or wrong input. Never set once any
   * signal was seen. A headset that gates noise sends silence for minutes when
   * nobody speaks into it; that is not a fault.
   */
  micNoSignal: boolean;
}

export const initialMeterState: MeterState = { mic: { kind: "unknown" }, system: { kind: "unknown" }, lastEventAt: null, micSeenSignal: false, micNoSignal: false };

const finite = (value: unknown): number => (typeof value === "number" && Number.isFinite(value) ? value : 0);

function nextSource(previous: SourceState, reading: SourceLevelPayload | null | undefined, now: number): SourceState {
  if (reading === null) return { kind: "absent" };
  if (reading === undefined) return previous;
  const rms = finite(reading.rms);
  const peak = finite(reading.peak);
  const db = levelToDb(rms);
  const peakNow = Math.max(db, levelToDb(peak));
  const before = previous.kind === "level" ? previous : null;
  const silent = Math.max(rms, peak) < SILENT_PEAK;
  return {
    kind: "level",
    db,
    peakDb: holdPeak(before ? before.peakDb : null, peakNow),
    silentSince: silent ? (before?.silentSince ?? now) : null,
  };
}

const heardSound = (source: SourceState) => source.kind === "level" && source.silentSince === null;

const noSignal = (mic: SourceState, seenSignal: boolean, now: number) =>
  !seenSignal && mic.kind === "level" && mic.silentSince !== null && now - mic.silentSince >= MIC_NO_SIGNAL_AFTER_MS;

/** One `recording-levels` event. */
export function applyLevels(previous: MeterState, payload: RecordingLevelsPayload, now: number): MeterState {
  const mic = nextSource(previous.mic, payload?.mic, now);
  const system = nextSource(previous.system, payload?.system, now);
  const micSeenSignal = previous.micSeenSignal || heardSound(mic);
  return { mic, system, lastEventAt: now, micSeenSignal, micNoSignal: noSignal(mic, micSeenSignal, now) };
}

function silenced(source: SourceState, now: number): SourceState {
  if (source.kind !== "level") return source;
  return {
    kind: "level",
    db: LEVEL_FLOOR_DB,
    peakDb: holdPeak(source.peakDb, LEVEL_FLOOR_DB),
    silentSince: source.silentSince ?? now,
  };
}

/**
 * Called on a timer while recording. After SILENCE_AFTER_MS without an event the
 * captured sources read as silence. Returns `previous` itself when nothing changes.
 */
export function applySilence(previous: MeterState, now: number): MeterState {
  if (previous.lastEventAt === null || now - previous.lastEventAt < SILENCE_AFTER_MS) return previous;
  const mic = silenced(previous.mic, now);
  const system = silenced(previous.system, now);
  const micNoSignal = noSignal(mic, previous.micSeenSignal, now);
  const settled = (source: SourceState) => source.kind !== "level" || (source.db === LEVEL_FLOOR_DB && source.peakDb === LEVEL_FLOOR_DB && source.silentSince !== null);
  if (settled(previous.mic) && settled(previous.system) && previous.micNoSignal === micNoSignal) return previous;
  return { ...previous, mic, system, micNoSignal };
}

/** Pausing or resuming starts the readings again; nothing is shown as silence. */
export function resetMeter(): MeterState {
  return initialMeterState;
}
