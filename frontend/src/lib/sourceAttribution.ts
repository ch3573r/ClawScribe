"use client";

import { invoke } from "@tauri-apps/api/core";
import {
  getBehaviorSettings,
  mirrorLegacyBehavior,
  refreshBehaviorSettings,
  subscribeBehaviorSettings,
} from "@/lib/behaviorSettings";

// Beta (experimental, default off): energy-based "Me" / "Participants" source
// attribution for live transcripts. The heuristic isn't reliable yet, so it's
// opt-in. The core owns the setting and applies it to segments transcribed
// after the change.

interface RecordingPreferences {
  auto_save?: boolean;
}

/** The core's value; off until it has been read. */
export function getSourceAttribution(): boolean {
  return getBehaviorSettings().sourceAttribution === true;
}

export async function getRecordingAudioSavingEnabled(): Promise<boolean> {
  try {
    const preferences = await invoke<RecordingPreferences>("get_recording_preferences");
    return preferences.auto_save !== false;
  } catch {
    return true;
  }
}

async function store(enabled: boolean): Promise<boolean> {
  await invoke("set_source_attribution_enabled", { enabled });
  // LEGACY mirror, so a downgrade to 0.62.0 sees the current value.
  mirrorLegacyBehavior("sourceAttribution", enabled);
  return (await refreshBehaviorSettings()).sourceAttribution === true;
}

/** Stores the setting in the core. Throws when the core could not save it. */
export async function setSourceAttribution(enabled: boolean): Promise<boolean> {
  return store(enabled && await getRecordingAudioSavingEnabled());
}

export function subscribeSourceAttribution(
  listener: (enabled: boolean) => void,
): () => void {
  return subscribeBehaviorSettings(() => listener(getSourceAttribution()));
}

/**
 * Source attribution depends on saved audio: without it the setting is turned
 * off in the core. Only a change is written.
 */
export async function refreshSourceAttributionAvailability(): Promise<boolean> {
  const stored = getSourceAttribution();
  const available = stored && await getRecordingAudioSavingEnabled();
  if (available !== stored) return store(available);
  return stored;
}
