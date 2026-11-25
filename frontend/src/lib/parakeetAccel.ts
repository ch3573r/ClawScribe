"use client";

import { invoke } from "@tauri-apps/api/core";
import {
  getBehaviorSettings,
  mirrorLegacyBehavior,
  refreshBehaviorSettings,
} from "@/lib/behaviorSettings";

// DirectML (Windows GPU) always runs in a separate supervised process,
// which can also be enabled for CPU transcription. The core owns both settings: it
// stores them, applies them at start-up before a recording can start, and
// applies a change on the next model load. DirectML has no effect unless the
// build includes the `directml` feature.

export interface ParakeetBackendSettings {
  directml: boolean;
  directmlAutomatic: boolean;
  directmlLastLoadFellBack: boolean;
  worker: boolean;
  directmlSessionDisabled: boolean;
  directmlDefaultUnavailable: boolean;
}

/** What the core runs right now; the switches show this. */
export async function getParakeetBackendSettings(): Promise<ParakeetBackendSettings> {
  const [directml, worker] = await Promise.all([
    invoke<{
      requested: boolean;
      automatic: boolean;
      lastLoadFellBack: boolean;
      defaultUnavailable: boolean;
      sessionDisabled: boolean;
    }>("get_parakeet_directml_status"),
    invoke<boolean>("get_parakeet_use_worker"),
  ]);
  return {
    directml: directml.requested,
    directmlAutomatic: directml.automatic,
    directmlLastLoadFellBack: directml.lastLoadFellBack,
    worker,
    directmlSessionDisabled: directml.sessionDisabled,
    directmlDefaultUnavailable: directml.defaultUnavailable,
  };
}

/** The stored preference as the core holds it; off until it has been read. */
export function getParakeetDirectml(): boolean {
  return getBehaviorSettings().parakeetDirectml === true;
}

export function getParakeetWorker(): boolean {
  return getBehaviorSettings().parakeetWorker === true;
}

/** Stores the change in the core. Throws when the core refused or could not save it. */
export async function setParakeetDirectml(enabled: boolean): Promise<void> {
  await invoke("set_parakeet_use_directml", { enabled });
  // LEGACY mirror, so a downgrade to 0.62.0 sees the current value.
  mirrorLegacyBehavior("parakeetDirectml", enabled);
  await refreshBehaviorSettings();
}

export async function setParakeetWorker(enabled: boolean): Promise<void> {
  // A busy recorder can reject the change; the control shows the error and the
  // value stays what the core holds.
  await invoke("set_parakeet_use_worker", { enabled });
  mirrorLegacyBehavior("parakeetWorker", enabled);
  await refreshBehaviorSettings();
}
