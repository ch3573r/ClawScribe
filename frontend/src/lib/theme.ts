"use client"

export type ThemePreference = "light" | "dark" | "system"
export type ResolvedTheme = "light" | "dark"

export const THEME_STORAGE_KEY = "clawscribe.theme"

export const themePreferences: ThemePreference[] = ["light", "dark", "system"]

const isThemePreference = (value: string | null): value is ThemePreference =>
  value === "light" || value === "dark" || value === "system"

export function getStoredThemePreference(): ThemePreference {
  if (typeof window === "undefined") return "system"

  try {
    const stored = window.localStorage.getItem(THEME_STORAGE_KEY)
    return isThemePreference(stored) ? stored : "system"
  } catch {
    return "system"
  }
}

export function getSystemTheme(): ResolvedTheme {
  if (typeof window === "undefined") return "light"

  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"
}

export function resolveThemePreference(preference: ThemePreference): ResolvedTheme {
  return preference === "system" ? getSystemTheme() : preference
}

export function applyThemePreference(preference: ThemePreference): ResolvedTheme {
  const resolvedTheme = resolveThemePreference(preference)

  if (typeof document !== "undefined") {
    const root = document.documentElement
    root.classList.toggle("dark", resolvedTheme === "dark")
    root.dataset.theme = resolvedTheme
    root.dataset.themePreference = preference
    root.style.colorScheme = resolvedTheme
  }

  return resolvedTheme
}

export async function applyNativeThemePreference(preference: ThemePreference): Promise<void> {
  if (typeof window === "undefined") return

  try {
    const { invoke } = await import("@tauri-apps/api/core")
    await invoke("set_native_theme", { theme: preference })
  } catch {
    // Browser previews do not have Tauri's native window API.
  }
}

export function setThemePreference(preference: ThemePreference): ResolvedTheme {
  if (typeof window !== "undefined") {
    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, preference)
    } catch {
      // Theme changes should still apply even if storage is unavailable.
    }
  }

  return applyThemePreference(preference)
}

// ── Accent color ───────────────────────────────────────────────────────────
// Overrides the --primary token (and its foreground/ring) so the user can pick
// an accent. Values are HSL component triples matching the CSS tokens. `id`
// "default" clears the override and falls back to the theme's built-in accent.

export const ACCENT_STORAGE_KEY = "clawscribe.accent"

export interface AccentColor {
  id: string
  name: string
  /** HSL components, e.g. "203 100% 26%". */
  primary: string
  /** Readable text on the accent. */
  foreground: string
}

export const DEFAULT_ACCENT_ID = "blue"

// Each swatch is an explicit accent applied in BOTH light and dark, so the
// choice always shows (no falling back to a theme default). Values are tuned to
// read on light and dark backgrounds with the given foreground.
export const accentColors: AccentColor[] = [
  { id: "blue", name: "Blue", primary: "203 100% 42%", foreground: "0 0% 100%" },
  { id: "teal", name: "Teal", primary: "166 55% 42%", foreground: "0 0% 100%" },
  { id: "sky", name: "Sky", primary: "199 89% 48%", foreground: "0 0% 100%" },
  { id: "violet", name: "Violet", primary: "262 60% 58%", foreground: "0 0% 100%" },
  { id: "magenta", name: "Magenta", primary: "329 75% 52%", foreground: "0 0% 100%" },
  { id: "emerald", name: "Emerald", primary: "152 55% 42%", foreground: "0 0% 100%" },
  { id: "amber", name: "Amber", primary: "38 92% 50%", foreground: "0 0% 10%" },
]

export function getStoredAccentId(): string {
  if (typeof window === "undefined") return DEFAULT_ACCENT_ID
  try {
    return window.localStorage.getItem(ACCENT_STORAGE_KEY) ?? DEFAULT_ACCENT_ID
  } catch {
    return DEFAULT_ACCENT_ID
  }
}

export function applyAccent(id: string): void {
  if (typeof document === "undefined") return
  const root = document.documentElement
  const accent =
    accentColors.find((a) => a.id === id) ??
    accentColors.find((a) => a.id === DEFAULT_ACCENT_ID)!
  root.style.setProperty("--primary", accent.primary)
  root.style.setProperty("--primary-foreground", accent.foreground)
  root.style.setProperty("--ring", accent.primary)
}

export function setAccent(id: string): void {
  if (typeof window !== "undefined") {
    try {
      window.localStorage.setItem(ACCENT_STORAGE_KEY, id)
    } catch {
      // Apply even if storage is unavailable.
    }
  }
  applyAccent(id)
}

export function subscribeToSystemTheme(callback: () => void): () => void {
  if (typeof window === "undefined") return () => undefined

  const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)")
  mediaQuery.addEventListener("change", callback)

  return () => mediaQuery.removeEventListener("change", callback)
}
