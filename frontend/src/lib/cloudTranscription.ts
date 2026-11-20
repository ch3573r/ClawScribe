"use client";

import { invoke } from "@tauri-apps/api/core";
import {
  getBehaviorSettings,
  mirrorLegacyBehavior,
  refreshBehaviorSettings,
  subscribeBehaviorSettings,
} from "@/lib/behaviorSettings";

// Beta (opt-in, default off): hosted whole-file transcription for import and
// retranscription. The core owns this setting; it reads as off until the core
// has answered, and a failed or unreadable value never switches it on.

/** The core's value; off until it has been read. */
export function getCloudTranscription(): boolean {
  return getBehaviorSettings().cloudTranscription === true;
}

/**
 * Stores the opt-in in the core. Throws when the core could not save it; the
 * value then stays what the core holds.
 */
export async function setCloudTranscription(enabled: boolean): Promise<boolean> {
  await invoke("set_cloud_transcription_enabled", { enabled });
  // LEGACY mirror, so a downgrade to 0.62.0 sees the current value.
  mirrorLegacyBehavior("cloudTranscription", enabled);
  return (await refreshBehaviorSettings()).cloudTranscription === true;
}

export function subscribeCloudTranscription(
  listener: (enabled: boolean) => void,
): () => void {
  return subscribeBehaviorSettings(() => listener(getCloudTranscription()));
}
