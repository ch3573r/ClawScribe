"use client";

import { invoke } from "@tauri-apps/api/core";

// Beta (experimental): route Nemotron inference through DirectML (Windows GPU).
// On Windows, ONNX models can't use the Vulkan build (Vulkan only accelerates
// whisper.cpp); DirectML is the GPU path for ONNX Runtime. Persisted locally and
// pushed to the backend, which applies it on the next model load. No effect
// unless the build includes the `directml` feature.

const KEY = "clawscribe.nemotronDirectml";

export function getNemotronDirectml(): boolean {
  if (typeof window === "undefined") return false;
  try {
    return window.localStorage.getItem(KEY) === "true";
  } catch {
    return false;
  }
}

export async function setNemotronDirectml(enabled: boolean): Promise<void> {
  if (typeof window !== "undefined") {
    try {
      window.localStorage.setItem(KEY, String(enabled));
    } catch {
      // best-effort
    }
  }
  try {
    await invoke("set_nemotron_use_directml", { enabled });
  } catch {
    // Backend may not expose it (older build / non-Windows); ignore.
  }
}

/** Push the stored preference to the backend. Call once at app startup. */
export async function applyNemotronDirectml(): Promise<void> {
  try {
    await invoke("set_nemotron_use_directml", { enabled: getNemotronDirectml() });
  } catch {
    // ignore
  }
}
