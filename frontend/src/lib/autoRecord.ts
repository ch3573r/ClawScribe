"use client";

export const AUTO_RECORD_STORAGE_KEY = "clawscribe.autoRecordOnTeams";
export const CALL_SETTINGS_CHANGED = "clawscribe-autorecord-changed";
const SETTINGS = "clawscribe.callDetection.v1";
const LEGACY_MODE = "clawscribe.teamsDetectionMode";
export const CALL_PROVIDERS = {
  teams: "Microsoft Teams", zoom: "Zoom", webex: "Webex", googleMeet: "Google Meet",
} as const;
export type CallProvider = keyof typeof CALL_PROVIDERS;
export type TeamsDetectionMode = "off" | "prompt" | "auto";
export type CallSettings = { mode: TeamsDetectionMode; providers: CallProvider[]; autoStop: boolean };
export type CallWindow = { provider: CallProvider; window: string; key: string };
export type CallScan = { supported: boolean; complete: boolean; active: CallWindow[]; ended: CallWindow[]; openWindows: string[] };
const defaults = (): CallSettings => ({ mode: "off", providers: ["teams"], autoStop: false });
const validMode = (value: unknown): value is TeamsDetectionMode => value === "auto" || value === "prompt" || value === "off";

export function getCallSettings(): CallSettings {
  try {
    const stored = window.localStorage.getItem(SETTINGS);
    if (stored !== null) {
      const data = JSON.parse(stored);
      if (!validMode(data?.mode) || !Array.isArray(data.providers)) return defaults();
      return {
        mode: data.mode,
        providers: (Object.keys(CALL_PROVIDERS) as CallProvider[]).filter(key => data.providers.includes(key)),
        autoStop: data.autoStop === true,
      };
    }
    const legacy = window.localStorage.getItem(LEGACY_MODE);
    const mode = validMode(legacy) ? legacy : window.localStorage.getItem(AUTO_RECORD_STORAGE_KEY) === "true" ? "auto" : "off";
    return { ...defaults(), mode };
  } catch { return defaults(); }
}

export function setCallSettings(settings: CallSettings) {
  window.localStorage.setItem(SETTINGS, JSON.stringify(settings));
  window.dispatchEvent(new Event(CALL_SETTINGS_CHANGED));
}

export function getTeamsDetectionMode() { return getCallSettings().mode; }
export function setTeamsDetectionMode(mode: TeamsDetectionMode) { setCallSettings({ ...getCallSettings(), mode }); }
export function getAutoRecordEnabled() { return getTeamsDetectionMode() === "auto"; }
