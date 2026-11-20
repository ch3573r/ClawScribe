"use client";

import { invoke } from "@tauri-apps/api/core";

/**
 * Settings that change how the core behaves are owned by the core: it stores
 * them, applies them at start-up before anything can start a recording, and
 * answers `get_behavior_settings`. This module reads them once, keeps the
 * latest values for the interface, and hands over the old webview values.
 *
 * The cloud transcription opt-in fails closed: until the core has answered, or
 * when it cannot, every value reads as its default and cloud transcription as off.
 */

export interface BehaviorSettings {
  parakeetDirectml: boolean;
  parakeetWorker: boolean;
  cloudTranscription: boolean;
  sourceAttribution: boolean;
  language: string;
  /** Whether the "recording started" notice is shown (the core's value; default yes). */
  showRecordingNotification: boolean;
  /** Whether the old `localStorage` values were already handed to the core. */
  legacyMigrated: boolean;
}

export const DEFAULT_BEHAVIOR_SETTINGS: BehaviorSettings = {
  parakeetDirectml: false,
  parakeetWorker: false,
  cloudTranscription: false,
  sourceAttribution: false,
  language: "auto",
  showRecordingNotification: true,
  legacyMigrated: false,
};

/**
 * LEGACY. Where the webview kept these settings up to 0.62.0. They are read
 * once for the hand-over to the core and are still written on every change so
 * that a downgrade to 0.62.0 finds the current values. They are never the
 * source of truth any more, and they are not deleted in this release.
 */
export const LEGACY_BEHAVIOR_KEYS = {
  parakeetDirectml: "clawscribe.parakeetDirectml",
  parakeetWorker: "clawscribe.parakeetWorker",
  cloudTranscription: "clawscribe.cloudTranscription",
  sourceAttribution: "clawscribe.sourceAttribution",
  language: "primaryLanguage",
} as const;

export interface LegacyBehaviorSettings {
  parakeetDirectml: boolean | null;
  parakeetWorker: boolean | null;
  cloudTranscription: boolean | null;
  sourceAttribution: boolean | null;
  language: string | null;
}

type ReadableStorage = Pick<Storage, "getItem">;

function legacyStorage(): ReadableStorage | null {
  try {
    return typeof window !== "undefined" ? window.localStorage : null;
  } catch {
    return null;
  }
}

/** The old values, `null` for one the webview never stored. Nothing else is read. */
export function readLegacyBehaviorSettings(storage: ReadableStorage | null = legacyStorage()): LegacyBehaviorSettings {
  const read = (key: string): string | null => {
    try {
      return storage ? storage.getItem(key) : null;
    } catch {
      return null;
    }
  };
  const flag = (key: string): boolean | null => {
    const value = read(key);
    return value === "true" ? true : value === "false" ? false : null;
  };
  const language = read(LEGACY_BEHAVIOR_KEYS.language);
  return {
    parakeetDirectml: flag(LEGACY_BEHAVIOR_KEYS.parakeetDirectml),
    parakeetWorker: flag(LEGACY_BEHAVIOR_KEYS.parakeetWorker),
    cloudTranscription: flag(LEGACY_BEHAVIOR_KEYS.cloudTranscription),
    sourceAttribution: flag(LEGACY_BEHAVIOR_KEYS.sourceAttribution),
    language: language && language.trim() ? language.trim() : null,
  };
}

/** Writes the legacy mirror (best effort) so a downgrade sees the current value. */
export function mirrorLegacyBehavior(key: keyof typeof LEGACY_BEHAVIOR_KEYS, value: boolean | string): void {
  try {
    if (typeof window !== "undefined") window.localStorage.setItem(LEGACY_BEHAVIOR_KEYS[key], String(value));
  } catch {
    // The mirror is a convenience for downgrades only.
  }
}

/** A value from the core, accepted only in its exact type. */
export function readBehaviorSettings(value: unknown): BehaviorSettings {
  const source = (value && typeof value === "object" ? value : {}) as Record<string, unknown>;
  const flag = (name: string) => source[name] === true;
  const language = typeof source.language === "string" && source.language.trim() ? source.language : DEFAULT_BEHAVIOR_SETTINGS.language;
  return {
    parakeetDirectml: flag("parakeetDirectml"),
    parakeetWorker: flag("parakeetWorker"),
    cloudTranscription: flag("cloudTranscription"),
    sourceAttribution: flag("sourceAttribution"),
    language,
    // Only an explicit false switches the notice off.
    showRecordingNotification: source.showRecordingNotification !== false,
    legacyMigrated: flag("legacyMigrated"),
  };
}

let current: BehaviorSettings | null = null;
let loading: Promise<BehaviorSettings> | null = null;
const listeners = new Set<() => void>();

function publish(next: BehaviorSettings) {
  current = next;
  for (const listener of [...listeners]) listener();
}

/** The latest values from the core, or the defaults (cloud off) until it has answered. */
export function getBehaviorSettings(): BehaviorSettings {
  return current ?? DEFAULT_BEHAVIOR_SETTINGS;
}

export function behaviorSettingsLoaded(): boolean {
  return current !== null;
}

export function subscribeBehaviorSettings(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * Reads the settings from the core. On the first run of this build the core
 * has not seen the old webview values yet: they are handed over once, and the
 * core keeps its own value wherever it already has one.
 */
export function loadBehaviorSettings(): Promise<BehaviorSettings> {
  if (current) return Promise.resolve(current);
  loading ??= (async () => {
    try {
      let settings = readBehaviorSettings(await invoke<unknown>("get_behavior_settings"));
      if (!settings.legacyMigrated) {
        // The core refuses an engine change while a job holds the slot: try again
        // for about fifteen seconds without holding anything up.
        for (let attempt = 0; ; attempt += 1) {
          try {
            settings = readBehaviorSettings(
              await invoke<unknown>("migrate_legacy_behavior_settings", { legacy: readLegacyBehaviorSettings() }),
            );
            break;
          } catch (error) {
            const busy = typeof error === "string" && (error.startsWith("PARAKEET_WORKER_BUSY:") || error.includes("job is active"));
            if (!busy || attempt >= 4) throw error;
            await new Promise<void>((resolve) => setTimeout(resolve, 3_750));
          }
        }
      }
      publish(settings);
      return settings;
    } finally {
      loading = null;
    }
  })();
  return loading;
}

/**
 * Whether the "recording started" notice is shown. It tells participants they
 * are being recorded, so it fails towards showing: a failed or slow read (more
 * than `timeoutMs`) is "show". It reads the core's value directly and does not
 * wait for, or depend on, the hand-over of the engine settings.
 */
export async function readNoticePreference(timeoutMs = 2_000): Promise<boolean> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const read = invoke<unknown>("get_behavior_settings").then(value => readBehaviorSettings(value).showRecordingNotification);
    const slow = new Promise<boolean>(resolve => { timer = setTimeout(() => resolve(true), timeoutMs); });
    return await Promise.race([read, slow]);
  } catch {
    return true;
  } finally {
    if (timer) clearTimeout(timer);
  }
}

/** Reads the values again from the core, after a change was accepted. */
export async function refreshBehaviorSettings(): Promise<BehaviorSettings> {
  const settings = readBehaviorSettings(await invoke<unknown>("get_behavior_settings"));
  publish(settings);
  return settings;
}

/** For tests: forget everything that was loaded. */
export function resetBehaviorSettingsForTests(): void {
  current = null;
  loading = null;
  listeners.clear();
}
