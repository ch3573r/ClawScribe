"use client";

import { invoke } from "@tauri-apps/api/core";

// Beta (experimental): route Parakeet inference through DirectML (Windows GPU).
// Persisted locally and pushed to the backend, which applies it on the next
// model load. No effect unless the build includes the `directml` feature.

const KEY = "clawscribe.parakeetDirectml";
const WORKER_KEY = "clawscribe.parakeetWorker";

export interface ParakeetBackendSettings {
  directml: boolean;
  worker: boolean;
  directmlSessionDisabled: boolean;
}

/** Stored values are preferences; only these backend reads drive the switches. */
export async function getParakeetBackendSettings(): Promise<ParakeetBackendSettings> {
  const [directml, worker, directmlSessionDisabled] = await Promise.all([
    invoke<boolean>("get_parakeet_use_directml"),
    invoke<boolean>("get_parakeet_use_worker"),
    invoke<boolean>("get_parakeet_directml_session_disabled"),
  ]);
  return { directml, worker, directmlSessionDisabled };
}

export function getParakeetDirectml(): boolean {
  if (typeof window === "undefined") return false;
  try {
    return window.localStorage.getItem(KEY) === "true";
  } catch {
    return false;
  }
}

export async function setParakeetDirectml(enabled: boolean): Promise<void> {
  if (typeof window !== "undefined") {
    try {
      window.localStorage.setItem(KEY, String(enabled));
    } catch {
      // best-effort
    }
  }
  try {
    await invoke("set_parakeet_use_directml", { enabled });
  } catch {
    // Backend may not expose it (older build / non-Windows); ignore.
  }
}

/** Push the stored preference to the backend. Call once at app startup. */
export async function applyParakeetDirectml(): Promise<void> {
  try {
    await invoke("set_parakeet_use_directml", { enabled: getParakeetDirectml() });
  } catch {
    // ignore
  }
}

export function getParakeetWorker(): boolean {
  if (typeof window === "undefined") return false;
  try {
    return window.localStorage.getItem(WORKER_KEY) === "true";
  } catch {
    return false;
  }
}

export async function setParakeetWorker(enabled: boolean): Promise<void> {
  // A busy recorder can reject the change. Persist only after it is accepted,
  // and let the control show the error instead of claiming the flag changed.
  await invoke("set_parakeet_use_worker", { enabled });
  if (typeof window !== "undefined") {
    try {
      window.localStorage.setItem(WORKER_KEY, String(enabled));
    } catch {
      // best-effort, matching the acceleration preference
    }
  }
}

export async function applyParakeetWorker(): Promise<void> {
  // Five attempts over about 15 s. Never hold up startup or interrupt an
  // active recording; only retry the backend's explicit admission refusal.
  for (let attempt = 0; attempt < 5; attempt += 1) {
    try {
      // Read again so a later attempt cannot restore a stale startup choice.
      await invoke("set_parakeet_use_worker", { enabled: getParakeetWorker() });
      return;
    } catch (error) {
      if (
        typeof error !== "string" ||
        !error.startsWith("PARAKEET_WORKER_BUSY:") ||
        attempt === 4
      ) {
        // Older backends and other failures are not retryable. Settings reads
        // backend truth and displays any unapplied stored preference.
        return;
      }
    }
    await new Promise<void>((resolve) => setTimeout(resolve, 3_750));
  }
}

export async function applyParakeetPreferences(): Promise<void> {
  // Both setters claim the model-changing job. Apply them sequentially.
  await applyParakeetDirectml();
  await applyParakeetWorker();
}
