"use client";

import { useEffect, useState } from "react";
import { Monitor, Moon, Sun } from "lucide-react";
import {
  accentColors,
  applyAccent,
  applyNativeThemePreference,
  applyThemePreference,
  getStoredAccentId,
  getStoredThemePreference,
  setAccent,
  setThemePreference,
  subscribeToSystemTheme,
  themePreferences,
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
    description: "Use the Kontron light palette",
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

export function ThemeInitializer() {
  useEffect(() => {
    const applyStoredTheme = () => {
      const storedPreference = getStoredThemePreference();
      applyThemePreference(storedPreference);
      void applyNativeThemePreference(storedPreference);
    };

    applyStoredTheme();
    applyAccent(getStoredAccentId());

    const unsubscribeSystemTheme = subscribeToSystemTheme(applyStoredTheme);
    window.addEventListener("storage", applyStoredTheme);

    return () => {
      unsubscribeSystemTheme();
      window.removeEventListener("storage", applyStoredTheme);
    };
  }, []);

  return null;
}

export function ThemeSettings() {
  const [preference, setPreference] = useState<ThemePreference>("system");
  const [accentId, setAccentId] = useState<string>("default");

  useEffect(() => {
    setAccentId(getStoredAccentId());
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

  return (
    <div className="rounded-xl border border-border bg-card p-6 shadow-xl shadow-black/20">
      <div className="mb-4">
        <h3 className="text-lg font-semibold text-foreground">Appearance</h3>
        <p className="mt-2 text-sm text-muted-foreground">
          Choose how ClawScribe follows light and dark mode.
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
              className={`flex min-h-24 flex-col items-start gap-3 rounded-lg border p-4 text-left transition-colors ${
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
    </div>
  );
}
