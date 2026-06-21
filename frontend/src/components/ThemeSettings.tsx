"use client";

import { useEffect, useState } from "react";
import { Monitor, Moon, Sun, Type } from "lucide-react";
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

const themeOptions: Record<
  ThemePreference,
  {
    label: string;
    description: string;
    Icon: typeof Sun;
  }
> = {
  light: {
    label: "Light",
    description: "Use the light palette",
    Icon: Sun,
  },
  dark: {
    label: "Dark",
    description: "Use the dark app palette",
    Icon: Moon,
  },
  system: {
    label: "System",
    description: "Follow the operating system",
    Icon: Monitor,
  },
};

const fontOptions: Record<FontPreference, { label: string; description: string }> = {
  "source-sans": {
    label: "Source Sans 3",
    description: "Bundled default. Compact and readable for dense notes.",
  },
  atkinson: {
    label: "Atkinson",
    description: "Bundled. Highly legible, wider, and more open.",
  },
  lexend: {
    label: "Lexend",
    description: "Bundled. Rounder and more spacious.",
  },
  fira: {
    label: "Fira Sans",
    description: "Bundled. Narrower with a stronger technical tone.",
  },
  plex: {
    label: "IBM Plex Sans",
    description: "Bundled. Structured and editorial.",
  },
  system: {
    label: "System",
    description: "Use the Windows/macOS/Linux interface font.",
  },
};

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

export function ThemeSettings() {
  const [preference, setPreference] = useState<ThemePreference>("system");
  const [accentId, setAccentId] = useState<string>("default");
  const [fontId, setFontId] = useState<FontPreference>("source-sans");

  useEffect(() => {
    setAccentId(getStoredAccentId());
    setFontId(getStoredFontPreference());
  }, []);

  const handleAccentChange = (id: string) => {
    setAccentId(id);
    setAccent(id);
  };

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

  const handlePreferenceChange = (nextPreference: ThemePreference) => {
    setPreference(nextPreference);
    setThemePreference(nextPreference);
    void applyNativeThemePreference(nextPreference);
  };

  const handleFontChange = (nextFont: FontPreference) => {
    setFontId(nextFont);
    setFontPreference(nextFont);
  };

  return (
    <div className="space-y-4">
      <section className="rounded-md border border-border bg-card p-6 shadow-sm">
        <div className="mb-4">
          <h3 className="text-lg font-semibold text-foreground">Theme</h3>
          <p className="mt-2 text-sm text-muted-foreground">
            Follow light, dark, or your system setting.
          </p>
        </div>

        <div
          className="grid gap-2 sm:grid-cols-3"
          role="radiogroup"
          aria-label="Theme preference"
        >
          {themePreferences.map((option) => {
            const { label, description, Icon } = themeOptions[option];
            const isSelected = option === preference;

            return (
              <button
                key={option}
                type="button"
                role="radio"
                aria-checked={isSelected}
                onClick={() => handlePreferenceChange(option)}
                className={`flex min-h-24 flex-col items-start gap-3 rounded-md border p-4 text-left transition-colors ${
                  isSelected
                    ? "border-primary/30 bg-primary/10 text-primary ring-1 ring-primary/50"
                    : "border-border bg-background text-muted-foreground hover:border-primary/70 hover:bg-muted"
                }`}
              >
                <span className="flex items-center gap-2 text-sm font-semibold">
                  <Icon className="h-4 w-4" />
                  {label}
                </span>
                <span className="text-xs leading-5">{description}</span>
              </button>
            );
          })}
        </div>

        <div className="mt-6">
          <h4 className="text-sm font-semibold text-foreground">Accent color</h4>
          <p className="mt-1 text-sm text-muted-foreground">
            Used for highlights, links, and primary buttons.
          </p>
          <div className="mt-3 flex flex-wrap gap-2" role="radiogroup" aria-label="Accent color">
            {accentColors.map((accent) => {
              const isSelected = accent.id === accentId;
              return (
                <button
                  key={accent.id}
                  type="button"
                  role="radio"
                  aria-checked={isSelected}
                  title={accent.name}
                  onClick={() => handleAccentChange(accent.id)}
                  className={`flex items-center gap-2 rounded-full border px-3 py-1.5 text-xs font-medium transition-colors ${
                    isSelected
                      ? "border-foreground/30 text-foreground ring-2 ring-offset-2 ring-offset-card"
                      : "border-border text-muted-foreground hover:bg-muted"
                  }`}
                  style={isSelected ? { boxShadow: `0 0 0 2px hsl(${accent.primary})` } : undefined}
                >
                  <span
                    className="h-3.5 w-3.5 rounded-full"
                    style={{ backgroundColor: `hsl(${accent.primary})` }}
                  />
                  {accent.name}
                </button>
              );
            })}
          </div>
        </div>
      </section>

      <section className="rounded-md border border-border bg-card p-6 shadow-sm">
        <div className="mb-4 flex items-start gap-3">
          <div className="flex h-9 w-9 items-center justify-center rounded-md border border-border bg-background text-primary">
            <Type className="h-4 w-4" />
          </div>
          <div>
            <h3 className="text-lg font-semibold text-foreground">Interface font</h3>
            <p className="mt-1 text-sm text-muted-foreground">
              Applies to navigation, settings, transcripts, and summaries.
            </p>
          </div>
        </div>

        <div className="grid gap-2 sm:grid-cols-2" role="radiogroup" aria-label="Interface font">
          {fontPreferences.map((font) => {
            const isSelected = font === fontId;
            const option = fontOptions[font];

            return (
              <button
                key={font}
                type="button"
                role="radio"
                aria-checked={isSelected}
                onClick={() => handleFontChange(font)}
                className={`rounded-md border p-4 text-left transition-colors ${
                  isSelected
                    ? "border-primary/40 bg-primary/10 text-foreground ring-1 ring-primary/50"
                    : "border-border bg-background text-muted-foreground hover:border-primary/60 hover:bg-muted"
                }`}
              >
                <div className="flex items-start justify-between gap-3">
                  <div>
                    <p className="text-sm font-semibold text-foreground">{option.label}</p>
                    <p className="mt-1 text-xs leading-5">{option.description}</p>
                  </div>
                  <span
                    className={`mt-0.5 h-2.5 w-2.5 rounded-full border ${
                      isSelected ? "border-primary bg-primary" : "border-muted-foreground/40"
                    }`}
                    aria-hidden="true"
                  />
                </div>
                <div
                  className="mt-4 rounded-md border border-border/70 bg-card px-3 py-2 text-foreground"
                  style={{ fontFamily: fontStacks[font] }}
                >
                  <p className="text-[17px] font-semibold leading-6">Meeting notes</p>
                  <p className="mt-1 text-sm leading-5 text-muted-foreground">
                    00:14 | Generate summary | Export to OneNote
                  </p>
                </div>
              </button>
            );
          })}
        </div>
      </section>
    </div>
  );
}
