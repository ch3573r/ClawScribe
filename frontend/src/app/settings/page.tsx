"use client";

import React, { useState, useEffect, useSyncExternalStore } from "react";
import {
  ArrowLeft,
  Settings2,
  ShieldCheck,
  Palette,
  Mic,
  Keyboard,
  AudioLines,
  Sparkles,
  BrainCircuit,
  CalendarClock,
  Bot,
  Activity,
} from "lucide-react";
import { useRouter } from "next/navigation";
import { invoke } from "@tauri-apps/api/core";
import { cn } from "@/lib/utils";
import { ConfluenceIcon, Microsoft365Icon } from "@/components/IntegrationIcons";
import { PreferenceSettings } from "@/components/PreferenceSettings";
import { PrivacySettings } from "@/components/settings/PrivacySettings";
import { ThemeSettings } from "@/components/ThemeSettings";
import { RecordingSettings } from "@/components/RecordingSettings";
import { KeyboardShortcutsSettings } from "@/components/KeyboardShortcutsSettings";
import { TranscriptionSettingsPage } from "@/components/settings/TranscriptionSettingsPage";
import { SummarySettings } from "@/components/settings/SummarySettings";
import { KnowledgeSettings } from "@/components/KnowledgeSettings";
import { Microsoft365Settings } from "@/components/settings/Microsoft365Settings";
import { CalendarsSettings } from "@/components/settings/CalendarsSettings";
import { ConfluenceSettings } from "@/components/settings/ConfluenceSettings";
import { AssistantAccessSettings } from "@/components/settings/AssistantAccessSettings";
import { DiagnosticsSettings } from "@/components/settings/DiagnosticsSettings";
import { resolveSettingsTab } from "@/components/settings/settings-tabs";
import { StatusDot, type StatusTone } from "@/components/ui/settings-layout";
import { useConfig } from "@/contexts/ConfigContext";
import { useMicrosoftExport } from "@/hooks/useMicrosoftExport";
import { getICalendarServerSnapshot, getICalendarSnapshot, subscribeToICalendars } from "@/services/icalService";

type SectionItem = {
  value: string;
  label: string;
  icon: React.ElementType;
  desc: string;
};

// Grouped so the nav reads as a structure, not a flat row of tabs. `value`s are
// stable (the sidebar deep-links via ?tab= and the open-settings-tab event).
const GROUPS: { label: string; items: SectionItem[] }[] = [
  {
    label: "Workspace",
    items: [
      { value: "general", label: "General", icon: Settings2, desc: "Updates, notifications and where recordings are stored" },
      { value: "privacy", label: "Privacy", icon: ShieldCheck, desc: "What stays on this PC and what leaves it" },
      { value: "appearance", label: "Appearance", icon: Palette, desc: "Theme, accent colour and interface font" },
      { value: "recording", label: "Recording", icon: Mic, desc: "Capture, audio devices and call detection" },
      { value: "shortcuts", label: "Shortcuts", icon: Keyboard, desc: "These work from any app, also when ClawScribe is in the background" },
    ],
  },
  {
    label: "Intelligence",
    items: [
      { value: "transcription", label: "Transcription", icon: AudioLines, desc: "Speech-to-text engine, model and options" },
      { value: "summary", label: "Summary", icon: Sparkles, desc: "Summary model, language, live assistance and templates" },
      { value: "memory", label: "Meeting memory", icon: BrainCircuit, desc: "Search across your meetings, on this PC" },
    ],
  },
  {
    label: "Connections",
    items: [
      { value: "ms365", label: "Microsoft 365", icon: Microsoft365Icon, desc: "Where exports go in OneNote, OneDrive, Planner and To Do" },
      { value: "calendars", label: "Calendars", icon: CalendarClock, desc: "Meeting titles and invitees for your recordings" },
      { value: "confluence", label: "Confluence", icon: ConfluenceIcon, desc: "Publish meeting notes as a Confluence page" },
      { value: "assistant", label: "Assistant access", icon: Bot, desc: "Let an MCP-compatible assistant read your saved meetings" },
    ],
  },
  {
    label: "Advanced",
    items: [
      { value: "diagnostics", label: "Diagnostics", icon: Activity, desc: "Build, engine and connection health for troubleshooting" },
    ],
  },
];

const ALL_ITEMS = GROUPS.flatMap((g) => g.items);

type NavDot = { tone: StatusTone; label: string };

export default function SettingsPage() {
  const router = useRouter();
  const { transcriptModelConfig, setTranscriptModelConfig } = useConfig();
  const [activeTab, setActiveTab] = useState("general");
  const microsoft = useMicrosoftExport();
  const calendars = useSyncExternalStore(subscribeToICalendars, getICalendarSnapshot, getICalendarServerSnapshot);

  useEffect(() => {
    const loadTranscriptConfig = async () => {
      try {
        const config = (await invoke("api_get_transcript_config")) as any;
        if (config) {
          setTranscriptModelConfig({
            provider: config.provider || "localWhisper",
            model: config.model || "large-v3",
            baseUrl: config.baseUrl || null,
            allowUnencrypted: config.allowUnencrypted ?? false,
            endpoint: config.endpoint || null,
            region: config.region || null,
          });
        }
      } catch (error) {
        console.error("Failed to load transcript config:", error);
      }
    };
    loadTranscriptConfig();
  }, [setTranscriptModelConfig]);

  // Deep-link support: ?tab=… on load, and the sidebar's open-settings-tab event
  // while already mounted.
  useEffect(() => {
    const requested = resolveSettingsTab(new URLSearchParams(window.location.search).get("tab"));
    if (requested) setActiveTab(requested);
    const onOpenTab = (e: Event) => {
      const tab = resolveSettingsTab((e as CustomEvent<string>).detail);
      if (tab) setActiveTab(tab);
    };
    window.addEventListener("open-settings-tab", onOpenTab as EventListener);
    return () => window.removeEventListener("open-settings-tab", onOpenTab as EventListener);
  }, []);

  const active = ALL_ITEMS.find((i) => i.value === activeTab) ?? ALL_ITEMS[0];

  // Status lamps for the Connections entries. They read state that hooks and
  // stores already hold, so they add no polling and no backend calls.
  const microsoftConnected = microsoft.connection.state === "connected";
  const microsoftCalendar = microsoftConnected && !microsoft.connection.unavailableExports?.includes("Calendar lookup");
  const calendarIssue = Object.keys(calendars.issues).length > 0;
  const dots: Record<string, NavDot> = {
    ms365: microsoftConnected ? { tone: "ok", label: "Connected" } : { tone: "off", label: "Signed out" },
    calendars: calendarIssue
      ? { tone: "warning", label: "A calendar has a problem" }
      : calendars.sources.length > 0 || microsoftCalendar
        ? { tone: "ok", label: "Calendars connected" }
        : { tone: "off", label: "No calendars" },
    confluence: { tone: "neutral", label: "Optional" },
  };

  const renderPanel = () => {
    switch (active.value) {
      case "general":
        return <PreferenceSettings />;
      case "privacy":
        return <PrivacySettings />;
      case "appearance":
        return <ThemeSettings />;
      case "recording":
        return <RecordingSettings />;
      case "shortcuts":
        return <KeyboardShortcutsSettings />;
      case "transcription":
        return <TranscriptionSettingsPage />;
      case "summary":
        return <SummarySettings />;
      case "memory":
        return <KnowledgeSettings />;
      case "ms365":
        return <Microsoft365Settings />;
      case "calendars":
        return <CalendarsSettings />;
      case "confluence":
        return <ConfluenceSettings />;
      case "assistant":
        return <AssistantAccessSettings />;
      case "diagnostics":
        return <DiagnosticsSettings />;
      default:
        return <PreferenceSettings />;
    }
  };

  return (
    <div className="flex h-full flex-col overflow-hidden bg-background text-foreground">
      {/* Top bar */}
      <header className="flex shrink-0 items-center gap-4 border-b border-border px-6 py-4">
        <button
          onClick={() => router.back()}
          className="flex items-center gap-2 rounded-md px-2.5 py-1.5 text-sm text-muted-foreground transition hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          <ArrowLeft className="h-4 w-4" />
          Back
        </button>
        <div className="h-5 w-px bg-border" />
        <h1 className="text-lg font-semibold tracking-tight">Settings</h1>
      </header>

      <div className="flex min-h-0 flex-1">
        {/* Left nav rail */}
        <nav aria-label="Settings sections" className="w-60 shrink-0 overflow-y-auto border-r border-border px-3 py-5">
          {GROUPS.map((group) => (
            <div key={group.label} className="mb-6">
              <p className="px-3 pb-2 text-xs text-muted-foreground">{group.label}</p>
              <div className="space-y-0.5">
                {group.items.map((item) => {
                  const Icon = item.icon;
                  const isActive = item.value === active.value;
                  const dot = dots[item.value];
                  return (
                    <button
                      key={item.value}
                      type="button"
                      onClick={() => setActiveTab(item.value)}
                      aria-current={isActive ? "page" : undefined}
                      className={cn(
                        "relative flex h-[34px] w-full items-center gap-2.5 rounded-md px-3 text-left text-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                        isActive
                          ? "bg-primary/10 text-foreground"
                          : "text-muted-foreground hover:bg-muted hover:text-foreground"
                      )}
                    >
                      {isActive && <span aria-hidden="true" className="absolute inset-y-1.5 left-0 w-[3px] rounded-full bg-primary" />}
                      <Icon className={cn("h-4 w-4 shrink-0", isActive && "text-primary")} />
                      <span className="min-w-0 flex-1 truncate">{item.label}</span>
                      {dot && (
                        <>
                          <StatusDot tone={dot.tone} />
                          <span className="sr-only">{dot.label}</span>
                        </>
                      )}
                    </button>
                  );
                })}
              </div>
            </div>
          ))}
        </nav>

        {/* Content pane */}
        <main className="min-w-0 flex-1 overflow-y-auto">
          <div className="w-full px-12 pb-14 pt-8">
            <div className="mb-6">
              <h2 className="text-2xl font-semibold tracking-tight">{active.label}</h2>
              <p className="mt-1 text-sm text-muted-foreground">{active.desc}</p>
            </div>
            {renderPanel()}
          </div>
        </main>
      </div>
    </div>
  );
}
