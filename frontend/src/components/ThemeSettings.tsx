"use client";

import { useEffect, useState, type KeyboardEvent } from "react";
import { Monitor, Moon, Sparkles, Sun } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Meta, SettingsCard, SettingsColumns, SettingsRow } from "@/components/ui/settings-layout";
import { cn } from "@/lib/utils";
import {
  accentColors,
  applyAccent,
  applyFontPreference,
  applyNativeThemePreference,
  applyThemePreference,
  fontPreferences,
  fontStacks,
  getStoredAccentId,
  getStoredFontPreference,
  getStoredThemePreference,
  setAccent,
  setFontPreference,
  setThemePreference,
  subscribeToSystemTheme,
  themePreferences,
  type FontPreference,
  type ThemePreference,
} from "@/lib/theme";

const themeOptions: Record<ThemePreference, { label: string; description: string; Icon: typeof Sun }> = {
  light: { label: "Light", description: "Always light", Icon: Sun },
  dark: { label: "Dark", description: "Always dark", Icon: Moon },
  system: { label: "System", description: "System follows Windows", Icon: Monitor },
};

const fontOptions: Record<FontPreference, { label: string; description: string }> = {
  "source-sans": { label: "Source Sans 3", description: "Default. Compact and readable for dense notes." },
  atkinson: { label: "Atkinson Hyperlegible", description: "Highly legible, wider and more open" },
  lexend: { label: "Lexend", description: "Rounder and more spacious" },
  fira: { label: "Fira Sans", description: "Narrower, with a technical tone" },
  plex: { label: "IBM Plex Sans", description: "Structured and editorial" },
  system: { label: "System", description: "Uses the Windows interface font" },
};

const PREVIEW_LINES = [
  { time: "00:14", text: "Let's start with the rollout status for the Vienna site." },
  { time: "00:31", text: "Imaging is done for 40 of 52 devices, the rest follow on Thursday." },
  { time: "01:02", text: "Then we freeze the image and only patch security fixes." },
];
const PREVIEW_ACTIONS = [
  "Send the revised rollout plan by Friday",
  "Confirm the Thursday imaging slot with the site team",
];

export function ThemeInitializer() {
  useEffect(() => {
    const applyStoredAppearance = () => {
      const storedPreference = getStoredThemePreference();
      applyThemePreference(storedPreference);
      void applyNativeThemePreference(storedPreference);
      applyAccent(getStoredAccentId());
      applyFontPreference(getStoredFontPreference());
    };

    applyStoredAppearance();

    const unsubscribeSystemTheme = subscribeToSystemTheme(applyStoredAppearance);
    window.addEventListener("storage", applyStoredAppearance);

    return () => {
      unsubscribeSystemTheme();
      window.removeEventListener("storage", applyStoredAppearance);
    };
  }, []);

  return null;
}

/** Arrow keys move the choice in a radio group; only the selected radio is a tab stop. */
function moveRadio(event: KeyboardEvent<HTMLElement>, current: number, count: number, select: (index: number) => void) {
  const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
  if (step === undefined) return;
  event.preventDefault();
  const next = (current + step + count) % count;
  select(next);
  const radios = event.currentTarget.closest<HTMLElement>("[role=radiogroup]")?.querySelectorAll<HTMLElement>("[role=radio]");
  radios?.[next]?.focus();
}

export function ThemeSettings() {
  const [preference, setPreference] = useState<ThemePreference>("system");
  const [accentId, setAccentId] = useState<string>("default");
  const [fontId, setFontId] = useState<FontPreference>("source-sans");

  useEffect(() => {
    setAccentId(getStoredAccentId());
    setFontId(getStoredFontPreference());
  }, []);

  useEffect(() => {
    const syncThemePreference = () => {
      const storedPreference = getStoredThemePreference();
      setPreference(storedPreference);
      applyThemePreference(storedPreference);
      setFontId(getStoredFontPreference());
      applyFontPreference(getStoredFontPreference());
    };

    syncThemePreference();

    const unsubscribeSystemTheme = subscribeToSystemTheme(syncThemePreference);
    window.addEventListener("storage", syncThemePreference);

    return () => {
      unsubscribeSystemTheme();
      window.removeEventListener("storage", syncThemePreference);
    };
  }, []);

  const handleAccentChange = (id: string) => {
    setAccentId(id);
    setAccent(id);
  };

  const handlePreferenceChange = (nextPreference: ThemePreference) => {
    setPreference(nextPreference);
    setThemePreference(nextPreference);
    void applyNativeThemePreference(nextPreference);
  };

  const handleFontChange = (nextFont: FontPreference) => {
    setFontId(nextFont);
    setFontPreference(nextFont);
  };

  const selectedAccent = accentColors.find((accent) => accent.id === accentId);
  const themeIndex = themePreferences.indexOf(preference);
  const accentIndex = Math.max(0, accentColors.findIndex((accent) => accent.id === accentId));
  const fontIndex = fontPreferences.indexOf(fontId);

  return (
    <SettingsColumns
      aside={
        <SettingsCard title="Preview" description="A transcript and its summary in your settings">
          <div className="grid gap-4 p-5 text-sm" style={{ fontFamily: fontStacks[fontId] }}>
            <div className="grid gap-3">
              {PREVIEW_LINES.map((line) => (
                <div key={line.time} className="grid grid-cols-[2.75rem_1fr] gap-3">
                  <span className="font-mono text-xs leading-5 tabular-nums text-primary">{line.time}</span>
                  <span className="leading-5 text-foreground">{line.text}</span>
                </div>
              ))}
            </div>
            <div className="grid gap-2">
              <h4 className="text-sm font-semibold text-foreground">Action items</h4>
              <ul className="grid list-disc gap-1.5 pl-5 text-[13px] leading-5 text-foreground marker:text-muted-foreground">
                {PREVIEW_ACTIONS.map((item) => <li key={item}>{item}</li>)}
              </ul>
            </div>
            {/* Sample controls only: inert so they cannot be focused or clicked. */}
            <div inert className="pointer-events-none flex flex-wrap items-center gap-2 select-none">
              <Button type="button" size="sm" tabIndex={-1}><Sparkles aria-hidden="true" />Generate summary</Button>
              <Button type="button" size="sm" variant="outline" tabIndex={-1}>Export</Button>
              <span className="px-2 text-[13px] text-primary">Open in OneNote</span>
            </div>
          </div>
        </SettingsCard>
      }
    >
      <SettingsCard>
        <SettingsRow title="Theme" description={themeOptions[preference].description}>
          <div role="radiogroup" aria-label="Theme" className="inline-flex gap-0.5 rounded-md border border-border bg-background p-0.5">
            {themePreferences.map((option, index) => {
              const { label, Icon } = themeOptions[option];
              const selected = option === preference;
              return (
                <button
                  key={option}
                  type="button"
                  role="radio"
                  aria-checked={selected}
                  tabIndex={selected ? 0 : -1}
                  onClick={() => handlePreferenceChange(option)}
                  onKeyDown={(event) => moveRadio(event, themeIndex, themePreferences.length, (i) => handlePreferenceChange(themePreferences[i]))}
                  className={cn(
                    "inline-flex h-8 items-center gap-1.5 rounded px-3 text-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                    selected ? "bg-muted text-foreground shadow-sm" : "text-muted-foreground hover:text-foreground",
                  )}
                >
                  <Icon aria-hidden="true" className="h-3.5 w-3.5" />
                  {label}
                </button>
              );
            })}
          </div>
        </SettingsRow>
        <SettingsRow
          title="Accent colour"
          description={`${selectedAccent?.name ?? "Default"} · used for selection, links and primary buttons`}
        >
          <div role="radiogroup" aria-label="Accent colour" className="flex max-w-[30rem] flex-wrap justify-end gap-2.5 p-1">
            {accentColors.map((accent, index) => {
              const selected = accent.id === accentId;
              return (
                <button
                  key={accent.id}
                  type="button"
                  role="radio"
                  aria-checked={selected}
                  aria-label={accent.name}
                  title={accent.name}
                  tabIndex={selected || (!selectedAccent && index === 0) ? 0 : -1}
                  onClick={() => handleAccentChange(accent.id)}
                  onKeyDown={(event) => moveRadio(event, accentIndex, accentColors.length, (i) => handleAccentChange(accentColors[i].id))}
                  className={cn(
                    "h-6 w-6 rounded-full transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-card",
                    selected && "ring-2 ring-foreground ring-offset-2 ring-offset-card",
                  )}
                  style={{ backgroundColor: `hsl(${accent.primary})` }}
                />
              );
            })}
          </div>
        </SettingsRow>
      </SettingsCard>

      <SettingsCard title="Interface font" description="Applies to navigation, settings, transcripts and summaries">
        <div role="radiogroup" aria-label="Interface font" className="divide-y divide-border/60">
          {fontPreferences.map((font) => {
            const selected = font === fontId;
            const option = fontOptions[font];
            return (
              <button
                key={font}
                type="button"
                role="radio"
                aria-checked={selected}
                tabIndex={selected ? 0 : -1}
                onClick={() => handleFontChange(font)}
                onKeyDown={(event) => moveRadio(event, fontIndex, fontPreferences.length, (i) => handleFontChange(fontPreferences[i]))}
                className="flex min-h-16 w-full items-center gap-4 px-5 py-3 text-left transition-colors hover:bg-muted/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
              >
                <span
                  aria-hidden="true"
                  className={cn(
                    "grid h-4 w-4 shrink-0 place-items-center rounded-full border",
                    selected ? "border-primary" : "border-muted-foreground/60",
                  )}
                >
                  {selected && <span className="h-2 w-2 rounded-full bg-primary" />}
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block text-sm text-foreground">{option.label}</span>
                  <span className="mt-0.5 block text-[13px] leading-5 text-muted-foreground">{option.description}</span>
                </span>
                {selected && <Meta>In use</Meta>}
              </button>
            );
          })}
        </div>
      </SettingsCard>
    </SettingsColumns>
  );
}
