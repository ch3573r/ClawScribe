"use client";

import { invoke } from "@tauri-apps/api/core";

// Beta (experimental, default off): energy-based "Me" / "Participants" source
// attribution for live transcripts. The heuristic isn't reliable yet, so it's
// opt-in. Persisted locally and pushed to the backend, which applies it to
// segments transcribed after the change.

const KEY = "clawscribe.sourceAttribution";
const CHANGE_EVENT = "clawscribe:source-attribution-change";

interface RecordingPreferences {
  auto_save?: boolean;
}

function persistSourceAttribution(enabled: boolean): void {
  if (typeof window === "undefined") return;

  try {
    window.localStorage.setItem(KEY, String(enabled));
  } catch {
    // best-effort
  }

  window.dispatchEvent(
    new CustomEvent(CHANGE_EVENT, { detail: { enabled } }),
  );
}

export function getSourceAttribution(): boolean {
  if (typeof window === "undefined") return false;
  try {
    return window.localStorage.getItem(KEY) === "true";
  } catch {
    return false;
  }
}

export async function getRecordingAudioSavingEnabled(): Promise<boolean> {
  try {
    const preferences = await invoke<RecordingPreferences>("get_recording_preferences");
    return preferences.auto_save !== false;
  } catch {
    return true;
  }
}

export async function setSourceAttribution(enabled: boolean): Promise<boolean> {
  const nextEnabled = enabled && await getRecordingAudioSavingEnabled();
  persistSourceAttribution(nextEnabled);

  try {
    await invoke("set_source_attribution_enabled", { enabled: nextEnabled });
  } catch {
    // Backend may not expose it (older build); ignore.
  }

  return nextEnabled;
}

export function subscribeSourceAttribution(
  listener: (enabled: boolean) => void,
): () => void {
  if (typeof window === "undefined") return () => undefined;

  const handleChange = (event: Event) => {
    const detail = (event as CustomEvent<{ enabled?: boolean }>).detail;
    listener(
      typeof detail?.enabled === "boolean"
        ? detail.enabled
        : getSourceAttribution(),
    );
  };

  const handleStorage = (event: StorageEvent) => {
    if (event.key === KEY || event.key === null) listener(getSourceAttribution());
  };

  window.addEventListener(CHANGE_EVENT, handleChange);
  window.addEventListener("storage", handleStorage);

  return () => {
    window.removeEventListener(CHANGE_EVENT, handleChange);
    window.removeEventListener("storage", handleStorage);
  };
}

export async function refreshSourceAttributionAvailability(): Promise<boolean> {
  const nextEnabled = getSourceAttribution() && await getRecordingAudioSavingEnabled();
  persistSourceAttribution(nextEnabled);

  try {
    await invoke("set_source_attribution_enabled", {
      enabled: nextEnabled,
    });
  } catch {
    // ignore
  }

  return nextEnabled;
}

/** Push the stored preference to the backend. Call once at app startup. */
export async function applySourceAttribution(): Promise<void> {
  await refreshSourceAttributionAvailability();
}
