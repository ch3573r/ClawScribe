"use client"

export type ThemePreference = "light" | "dark" | "system"
export type ResolvedTheme = "light" | "dark"

export const THEME_STORAGE_KEY = "clawscribe.theme"

export const themePreferences: ThemePreference[] = ["light", "dark", "system"]

const isThemePreference = (value: string | null): value is ThemePreference =>
  value === "light" || value === "dark" || value === "system"

function readStored(key: string): string | null {
  if (typeof window === "undefined") return null
  try {
    return window.localStorage.getItem(key)
  } catch {
    return null
  }
}

function writeStored(key: string, value: string): void {
  if (typeof window === "undefined") return
  try {
    window.localStorage.setItem(key, value)
  } catch {
    // The change still applies for this session when storage is unavailable.
  }
}

export function getStoredThemePreference(): ThemePreference {
  const stored = readStored(THEME_STORAGE_KEY)
  return isThemePreference(stored) ? stored : "system"
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
  writeStored(THEME_STORAGE_KEY, preference)
  return applyThemePreference(preference)
}

// ── Fonts ──────────────────────────────────────────────────────────────────
// The interface font applies everywhere. The reading font applies to transcript
// and summary text and follows the interface font unless one is chosen. Either
// can be a bundled family, the Windows interface font, or any family installed
// on this PC, given by name.

const SANS_FALLBACK = "ui-sans-serif, system-ui, sans-serif"
const SERIF_FALLBACK = 'ui-serif, Georgia, Cambria, "Times New Roman", serif'

export const bundledFonts = [
  { id: "source-sans", label: "Source Sans 3", description: "Default. Compact and readable for dense notes.", stack: `var(--font-source-sans-3), ${SANS_FALLBACK}` },
  { id: "inter", label: "Inter", description: "Neutral, with tall lowercase letters", stack: `var(--font-inter), ${SANS_FALLBACK}` },
  { id: "geist", label: "Geist", description: "Geometric and tight", stack: `var(--font-geist), ${SANS_FALLBACK}` },
  { id: "atkinson", label: "Atkinson Hyperlegible", description: "Highly legible, wider and more open", stack: `var(--font-atkinson), ${SANS_FALLBACK}` },
  { id: "lexend", label: "Lexend", description: "Rounder and more spacious", stack: `var(--font-lexend), ${SANS_FALLBACK}` },
  { id: "fira", label: "Fira Sans", description: "Narrower, with a technical tone", stack: `var(--font-fira-sans), ${SANS_FALLBACK}` },
  { id: "plex", label: "IBM Plex Sans", description: "Structured and editorial", stack: `var(--font-ibm-plex-sans), ${SANS_FALLBACK}` },
  { id: "source-serif", label: "Source Serif 4", description: "Serif, suited to long reading", stack: `var(--font-source-serif-4), ${SERIF_FALLBACK}` },
  { id: "system", label: "System", description: "Uses the Windows interface font", stack: 'ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif' },
] as const

export type FontPreference = (typeof bundledFonts)[number]["id"] | "custom"
export type ReadingFontPreference = FontPreference | "interface"

export const FONT_STORAGE_KEY = "clawscribe.font"
export const CUSTOM_FONT_STORAGE_KEY = "clawscribe.font.custom"
export const READING_FONT_STORAGE_KEY = "clawscribe.readingFont"
export const CUSTOM_READING_FONT_STORAGE_KEY = "clawscribe.readingFont.custom"

export const DEFAULT_FONT: FontPreference = "source-sans"
export const fontPreferences: FontPreference[] = [...bundledFonts.map((font) => font.id), "custom"]

const isFontPreference = (value: string | null): value is FontPreference =>
  fontPreferences.includes(value as FontPreference)

/** A family name safe to place inside a quoted CSS string. */
export function cleanFontName(name: string): string {
  return name.replace(/["'\\;{}()<>]/g, "").replace(/\s+/g, " ").trim().slice(0, 80)
}

/** The CSS font stack for a choice. An empty custom name falls back to the default font. */
export function fontStack(preference: FontPreference, customName = ""): string {
  if (preference === "custom") {
    const name = cleanFontName(customName)
    return name ? `"${name}", ${SANS_FALLBACK}` : bundledFonts[0].stack
  }
  return (bundledFonts.find((font) => font.id === preference) ?? bundledFonts[0]).stack
}

/**
 * Whether a family is installed on this PC. A missing family renders in the
 * generic fallback, so its text is exactly as wide as the fallback alone.
 */
export function isFontInstalled(name: string): boolean {
  const family = cleanFontName(name)
  if (!family || typeof document === "undefined") return false
  const context = document.createElement("canvas").getContext("2d")
  if (!context) return true
  const sample = "mmmmmmmmmmlli WQ@ 0123456789"
  const width = (font: string) => {
    context.font = `72px ${font}`
    return context.measureText(sample).width
  }
  return ["monospace", "serif", "sans-serif"].some((generic) => width(`"${family}", ${generic}`) !== width(generic))
}

export interface FontSettings {
  font: FontPreference
  customFont: string
  readingFont: ReadingFontPreference
  customReadingFont: string
}

export function getStoredFontSettings(): FontSettings {
  const font = readStored(FONT_STORAGE_KEY)
  const readingFont = readStored(READING_FONT_STORAGE_KEY)
  return {
    font: isFontPreference(font) ? font : DEFAULT_FONT,
    customFont: readStored(CUSTOM_FONT_STORAGE_KEY) ?? "",
    readingFont: isFontPreference(readingFont) ? readingFont : "interface",
    customReadingFont: readStored(CUSTOM_READING_FONT_STORAGE_KEY) ?? "",
  }
}

export function applyFontSettings(settings: FontSettings): void {
  if (typeof document === "undefined") return

  const root = document.documentElement
  root.dataset.fontPreference = settings.font
  root.style.setProperty("--font-app-sans", fontStack(settings.font, settings.customFont))
  if (settings.readingFont === "interface") root.style.removeProperty("--font-reading")
  else root.style.setProperty("--font-reading", fontStack(settings.readingFont, settings.customReadingFont))
}

export function setFontSettings(settings: FontSettings): void {
  writeStored(FONT_STORAGE_KEY, settings.font)
  writeStored(CUSTOM_FONT_STORAGE_KEY, cleanFontName(settings.customFont))
  writeStored(READING_FONT_STORAGE_KEY, settings.readingFont)
  writeStored(CUSTOM_READING_FONT_STORAGE_KEY, cleanFontName(settings.customReadingFont))
  applyFontSettings(settings)
}

// ── Text size ──────────────────────────────────────────────────────────────
// Scales the whole webview, like browser zoom, so text, controls and spacing
// stay in proportion.

export const TEXT_SIZE_STORAGE_KEY = "clawscribe.textSize"

export const textSizes = [
  { id: "small", label: "Small", zoom: 0.9 },
  { id: "default", label: "Default", zoom: 1 },
  { id: "large", label: "Large", zoom: 1.1 },
  { id: "larger", label: "Larger", zoom: 1.25 },
] as const

export type TextSize = (typeof textSizes)[number]["id"]

export function getStoredTextSize(): TextSize {
  const stored = readStored(TEXT_SIZE_STORAGE_KEY)
  return textSizes.some((size) => size.id === stored) ? (stored as TextSize) : "default"
}

export async function applyTextSize(size: TextSize): Promise<void> {
  if (typeof window === "undefined") return

  const zoom = textSizes.find((option) => option.id === size)?.zoom ?? 1
  try {
    const { getCurrentWebview } = await import("@tauri-apps/api/webview")
    await getCurrentWebview().setZoom(zoom)
  } catch {
    // Browser previews do not have Tauri's webview API.
  }
}

export function setTextSize(size: TextSize): void {
  writeStored(TEXT_SIZE_STORAGE_KEY, size)
  void applyTextSize(size)
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

export const DEFAULT_ACCENT_ID = "claw"

// Each swatch is an explicit accent applied in BOTH light and dark, so the
// choice always shows (no falling back to a theme default). Values are tuned to
// read on light and dark backgrounds with the given foreground. "claw" is the
// ClawScribe brand accent (the electric cyan-azure of the mark) and the default;
// the rest stay so the accent picker keeps its full range.
export const accentColors: AccentColor[] = [
  { id: "claw", name: "Claw", primary: "186 96% 50%", foreground: "195 70% 8%" },
  { id: "blue", name: "Blue", primary: "218 92% 56%", foreground: "0 0% 100%" },
  { id: "teal", name: "Teal", primary: "168 74% 38%", foreground: "0 0% 100%" },
  { id: "sky", name: "Sky", primary: "204 90% 54%", foreground: "0 0% 100%" },
  { id: "violet", name: "Violet", primary: "258 78% 64%", foreground: "0 0% 100%" },
  { id: "magenta", name: "Magenta", primary: "326 80% 56%", foreground: "0 0% 100%" },
  { id: "emerald", name: "Emerald", primary: "158 68% 40%", foreground: "0 0% 100%" },
  { id: "amber", name: "Amber", primary: "40 95% 52%", foreground: "30 45% 10%" },
  { id: "red", name: "Red", primary: "353 80% 56%", foreground: "0 0% 100%" },
  { id: "orange", name: "Orange", primary: "22 92% 54%", foreground: "0 0% 100%" },
  { id: "indigo", name: "Indigo", primary: "243 74% 64%", foreground: "0 0% 100%" },
  { id: "lime", name: "Lime", primary: "88 60% 42%", foreground: "84 50% 8%" },
]

export function getStoredAccentId(): string {
  return readStored(ACCENT_STORAGE_KEY) ?? DEFAULT_ACCENT_ID
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
  writeStored(ACCENT_STORAGE_KEY, id)
  applyAccent(id)
}

export function subscribeToSystemTheme(callback: () => void): () => void {
  if (typeof window === "undefined") return () => undefined

  const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)")
  mediaQuery.addEventListener("change", callback)

  return () => mediaQuery.removeEventListener("change", callback)
}
