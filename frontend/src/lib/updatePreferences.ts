"use client";

import { invoke } from "@tauri-apps/api/core";

export const AUTO_UPDATE_CHECK_STORAGE_KEY = "clawscribe.autoUpdateCheck";
export const AUTO_UPDATE_CHECK_CHANGED_EVENT = "clawscribe:auto-update-check-changed";

let nativePreference: boolean | null = null;
let loading: Promise<boolean> | null = null;

function readLegacyPreference(): boolean | null {
  try {
    const value = window.localStorage.getItem(AUTO_UPDATE_CHECK_STORAGE_KEY);
    return value === "true" ? true : value === "false" ? false : null;
  } catch {
    return null;
  }
}

export function getAutoUpdateCheckEnabled(): boolean {
  return nativePreference ?? readLegacyPreference() ?? true;
}

export function loadAutoUpdateCheckEnabled(): Promise<boolean> {
  if (nativePreference !== null) return Promise.resolve(nativePreference);
  loading ??= invoke<unknown>("load_auto_update_preference", { legacyEnabled: readLegacyPreference() })
    .then(enabled => {
      if (typeof enabled === "boolean") nativePreference ??= enabled;
      return getAutoUpdateCheckEnabled();
    })
    .catch(getAutoUpdateCheckEnabled)
    .finally(() => { loading = null; });
  return loading;
}

export async function setAutoUpdateCheckEnabled(enabled: boolean): Promise<void> {
  await invoke("set_auto_update_preference", { enabled });
  nativePreference = enabled;
  try {
    window.localStorage.setItem(AUTO_UPDATE_CHECK_STORAGE_KEY, String(enabled));
  } catch {
    // This copy is for older builds. The native save already succeeded.
  }

  window.dispatchEvent(
    new CustomEvent<boolean>(AUTO_UPDATE_CHECK_CHANGED_EVENT, {
      detail: enabled,
    }),
  );
}
