"use client";

import { invoke } from "@tauri-apps/api/core";

const KEY = "clawscribe.cloudTranscription";
const CHANGE_EVENT = "clawscribe:cloud-transcription-change";

function persistCloudTranscription(enabled: boolean): void {
  if (typeof window === "undefined") return;

  try {
    window.localStorage.setItem(KEY, String(enabled));
  } catch {
    // best-effort
  }

  window.dispatchEvent(new CustomEvent(CHANGE_EVENT, { detail: { enabled } }));
}

export function getCloudTranscription(): boolean {
  if (typeof window === "undefined") return false;
  try {
    return window.localStorage.getItem(KEY) === "true";
  } catch {
    return false;
  }
}

export async function setCloudTranscription(enabled: boolean): Promise<boolean> {
  persistCloudTranscription(enabled);

  try {
    await invoke("set_cloud_transcription_enabled", { enabled });
  } catch {
    // Backend may not expose it in older builds; keep local preference.
  }

  return enabled;
}

export function subscribeCloudTranscription(
  listener: (enabled: boolean) => void,
): () => void {
  if (typeof window === "undefined") return () => undefined;

  const handleChange = (event: Event) => {
    const detail = (event as CustomEvent<{ enabled?: boolean }>).detail;
    listener(
      typeof detail?.enabled === "boolean"
        ? detail.enabled
        : getCloudTranscription(),
    );
  };

  const handleStorage = (event: StorageEvent) => {
    if (event.key === KEY || event.key === null) listener(getCloudTranscription());
  };

  window.addEventListener(CHANGE_EVENT, handleChange);
  window.addEventListener("storage", handleStorage);

  return () => {
    window.removeEventListener(CHANGE_EVENT, handleChange);
    window.removeEventListener("storage", handleStorage);
  };
}

export async function applyCloudTranscription(): Promise<void> {
  await setCloudTranscription(getCloudTranscription());
}
