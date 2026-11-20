/** Settings tab values. Deep links (?tab=) and the open-settings-tab event use them. */
export const SETTINGS_TABS = [
  "general", "privacy", "appearance", "recording", "shortcuts",
  "transcription", "summary", "memory",
  "ms365", "calendars", "confluence", "assistant",
  "diagnostics",
] as const;

// Tab values that older links and callers still use.
const TAB_ALIASES: Record<string, string> = {
  integrations: "ms365",
  beta: "transcription",
};

/** Map a requested ?tab= or open-settings-tab value to a real tab, or null. */
export function resolveSettingsTab(requested: string | null | undefined): string | null {
  if (!requested) return null;
  const value = TAB_ALIASES[requested] ?? requested;
  return (SETTINGS_TABS as readonly string[]).includes(value) ? value : null;
}
