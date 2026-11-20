"use client";

import { useEffect, useMemo, useState, type KeyboardEvent } from "react";
import { Monitor, Moon, Sparkles, Sun } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Meta, SettingsCard, SettingsColumns, SettingsRow, StatusText } from "@/components/ui/settings-layout";
import { useDraftCommit } from "@/hooks/useDraftCommit";
import { cn } from "@/lib/utils";
import {
  accentColors,
  applyAccent,
  applyFontSettings,
  applyNativeThemePreference,
  applyTextSize,
  applyThemePreference,
  bundledFonts,
  cleanFontName,
  DEFAULT_FONT,
  fontPreferences,
  fontStack,
  getStoredAccentId,
  getStoredFontSettings,
  getStoredTextSize,
  getStoredThemePreference,
  isFontInstalled,
  setAccent,
  setFontSettings,
  setTextSize,
  setThemePreference,
  subscribeToSystemTheme,
  textSizes,
  themePreferences,
  type FontSettings,
  type ReadingFontPreference,
  type TextSize,
  type ThemePreference,
} from "@/lib/theme";

const themeOptions: Record<ThemePreference, { label: string; description: string; Icon: typeof Sun }> = {
  light: { label: "Light", description: "Always light", Icon: Sun },
  dark: { label: "Dark", description: "Always dark", Icon: Moon },
  system: { label: "System", description: "System follows Windows", Icon: Monitor },
};

const CUSTOM_FONT = { label: "Installed font", description: "Any font installed on this PC, by name" };

const DEFAULT_FONT_SETTINGS: FontSettings = { font: DEFAULT_FONT, customFont: "", readingFont: "interface", customReadingFont: "" };

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
      applyFontSettings(getStoredFontSettings());
    };

    applyStoredAppearance();
    void applyTextSize(getStoredTextSize());

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

const segmentClasses = (selected: boolean) => cn(
  "inline-flex h-8 items-center gap-1.5 rounded px-3 text-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
  selected ? "bg-muted text-foreground shadow-sm" : "text-muted-foreground hover:text-foreground",
);

/**
 * Name field for a font installed on this PC, with a note when Windows does not
 * have it. The typed text stays here; the font is applied, stored and checked
 * when typing pauses (500 ms), on Enter and on leaving the field, so the page
 * does not flicker through partial names.
 */
function InstalledFontField({ value, onChange, className }: { value: string; onChange: (name: string) => void; className?: string }) {
  const { draft, setDraft, commit, pending } = useDraftCommit(value, onChange);
  const committedName = cleanFontName(value);
  const installed = useMemo(() => committedName === "" || isFontInstalled(committedName), [committedName]);
  const missing = !pending && !installed;
  return (
    <div className={cn("grid gap-2", className)}>
      <Input
        value={draft}
        onChange={(event) => setDraft(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => { if (event.key === "Enter") commit(); }}
        aria-label="Font name"
        placeholder="For example Calibri"
        spellCheck={false}
        autoComplete="off"
        maxLength={80}
        className="max-w-sm"
      />
      {missing
        ? <StatusText tone="warning" className="whitespace-normal text-[13px]">Not found on this PC. The Windows font is used instead.</StatusText>
        : !cleanFontName(draft) && <p className="text-[13px] leading-5 text-muted-foreground">Type the name as Windows shows it under Settings, Personalisation, Fonts.</p>}
    </div>
  );
}

export function ThemeSettings() {
  const [preference, setPreference] = useState<ThemePreference>("system");
  const [accentId, setAccentId] = useState<string>("default");
  const [fonts, setFonts] = useState<FontSettings>(DEFAULT_FONT_SETTINGS);
  const [textSize, setTextSizeState] = useState<TextSize>("default");

  useEffect(() => {
    setAccentId(getStoredAccentId());
    setTextSizeState(getStoredTextSize());
  }, []);

  useEffect(() => {
    const syncThemePreference = () => {
      const storedPreference = getStoredThemePreference();
      setPreference(storedPreference);
      applyThemePreference(storedPreference);
      const storedFonts = getStoredFontSettings();
      setFonts(storedFonts);
      applyFontSettings(storedFonts);
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

  const handleFontsChange = (change: Partial<FontSettings>) => {
    const next = { ...fonts, ...change };
    setFonts(next);
    setFontSettings(next);
  };

  const handleTextSizeChange = (nextSize: TextSize) => {
    setTextSizeState(nextSize);
    setTextSize(nextSize);
  };

  const selectedAccent = accentColors.find((accent) => accent.id === accentId);
  const themeIndex = themePreferences.indexOf(preference);
  const accentIndex = Math.max(0, accentColors.findIndex((accent) => accent.id === accentId));
  const fontIndex = fontPreferences.indexOf(fonts.font);
  const textSizeIndex = textSizes.findIndex((size) => size.id === textSize);
  const interfaceStack = fontStack(fonts.font, fonts.customFont);
  const readingStack = fonts.readingFont === "interface" ? interfaceStack : fontStack(fonts.readingFont, fonts.customReadingFont);

  return (
    <SettingsColumns
      aside={
        <SettingsCard title="Preview" description="A transcript and its summary in your settings">
          <div className="grid gap-4 p-5 text-sm" style={{ fontFamily: interfaceStack }}>
            <div className="grid gap-3" style={{ fontFamily: readingStack }}>
              {PREVIEW_LINES.map((line) => (
                <div key={line.time} className="grid grid-cols-[2.75rem_1fr] gap-3">
                  <span className="font-mono text-xs leading-5 tabular-nums text-primary">{line.time}</span>
                  <span className="leading-5 text-foreground">{line.text}</span>
                </div>
              ))}
            </div>
            <div className="grid gap-2" style={{ fontFamily: readingStack }}>
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
            {themePreferences.map((option) => {
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
                  className={segmentClasses(selected)}
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
        <SettingsRow title="Text size" description="Scales text and controls together">
          <div role="radiogroup" aria-label="Text size" className="inline-flex gap-0.5 rounded-md border border-border bg-background p-0.5">
            {textSizes.map((size) => {
              const selected = size.id === textSize;
              return (
                <button
                  key={size.id}
                  type="button"
                  role="radio"
                  aria-checked={selected}
                  tabIndex={selected ? 0 : -1}
                  onClick={() => handleTextSizeChange(size.id)}
                  onKeyDown={(event) => moveRadio(event, textSizeIndex, textSizes.length, (i) => handleTextSizeChange(textSizes[i].id))}
                  className={segmentClasses(selected)}
                >
                  {size.label}
                </button>
              );
            })}
          </div>
        </SettingsRow>
      </SettingsCard>

      <SettingsCard title="Interface font" description="Used across the app">
        <div>
        <div role="radiogroup" aria-label="Interface font" className="divide-y divide-border/60">
          {fontPreferences.map((font) => {
            const selected = font === fonts.font;
            const option = bundledFonts.find((bundled) => bundled.id === font) ?? CUSTOM_FONT;
            return (
              <div key={font}>
                <button
                  type="button"
                  role="radio"
                  aria-checked={selected}
                  tabIndex={selected ? 0 : -1}
                  onClick={() => handleFontsChange({ font })}
                  onKeyDown={(event) => moveRadio(event, fontIndex, fontPreferences.length, (i) => handleFontsChange({ font: fontPreferences[i] }))}
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
                    {/* Each name is set in its own font, so the list doubles as a specimen. */}
                    <span className="block text-[15px] text-foreground" style={{ fontFamily: fontStack(font, fonts.customFont) }}>{option.label}</span>
                    <span className="mt-0.5 block text-[13px] leading-5 text-muted-foreground">{option.description}</span>
                  </span>
                  {selected && <Meta>In use</Meta>}
                </button>
              </div>
            );
          })}
        </div>
        {/* Outside the radio group: a text field is not one of its choices. The
            Installed font option is the last row, so this sits directly under it. */}
        {fonts.font === "custom" && (
          <InstalledFontField
            value={fonts.customFont}
            onChange={(customFont) => handleFontsChange({ customFont })}
            className="pb-4 pl-[3.25rem] pr-5"
          />
        )}
        </div>
      </SettingsCard>

      <SettingsCard>
        <SettingsRow title="Transcript and summary font" description="Long text can use its own font, such as a serif">
          <Select value={fonts.readingFont} onValueChange={(value) => handleFontsChange({ readingFont: value as ReadingFontPreference })}>
            <SelectTrigger aria-label="Transcript and summary font" className="w-60">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="interface">Same as interface</SelectItem>
              {bundledFonts.map((font) => (
                <SelectItem key={font.id} value={font.id} style={{ fontFamily: font.stack }}>{font.label}</SelectItem>
              ))}
              <SelectItem value="custom">{CUSTOM_FONT.label}</SelectItem>
            </SelectContent>
          </Select>
        </SettingsRow>
        {fonts.readingFont === "custom" && (
          <InstalledFontField
            value={fonts.customReadingFont}
            onChange={(customReadingFont) => handleFontsChange({ customReadingFont })}
            className="px-5 py-4"
          />
        )}
      </SettingsCard>
    </SettingsColumns>
  );
}
