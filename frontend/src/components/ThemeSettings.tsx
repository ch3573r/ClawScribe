"use client";

import { useEffect, useState } from "react";
import { Monitor, Moon, Sun } from "lucide-react";
import {
  applyNativeThemePreference,
  applyThemePreference,
  getStoredThemePreference,
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
                  ? "border-primary/30 bg-primary/10 text-primary ring-1 ring-cyan-300/50"
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
    </div>
  );
}
