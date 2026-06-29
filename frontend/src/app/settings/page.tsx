"use client";

import React, { useState, useEffect } from "react";
import {
  ArrowLeft,
  Settings2,
  Palette,
  Mic,
  Keyboard,
  AudioLines,
  Sparkles,
  Plug,
  Activity,
  FlaskConical,
} from "lucide-react";
import { useRouter } from "next/navigation";
import { invoke } from "@tauri-apps/api/core";
import { motion } from "framer-motion";
import { cn } from "@/lib/utils";
import { TranscriptSettings } from "@/components/TranscriptSettings";
import { RecordingSettings } from "@/components/RecordingSettings";
import { PreferenceSettings } from "@/components/PreferenceSettings";
import { SummaryModelSettings } from "@/components/SummaryModelSettings";
import { BetaSettings } from "@/components/BetaSettings";
import { IntegrationsSettings, DiagnosticsSettings } from "@/components/IntegrationsSettings";
import { ThemeSettings } from "@/components/ThemeSettings";
import { KeyboardShortcutsSettings } from "@/components/KeyboardShortcutsSettings";
import { useConfig } from "@/contexts/ConfigContext";

type SectionItem = {
  value: string;
  label: string;
  icon: typeof Settings2;
  desc: string;
};

// Grouped so the nav reads as a structure, not a flat row of tabs. `value`s are
// stable (the sidebar deep-links via ?tab= and the open-settings-tab event);
// "appearance" and "shortcuts" are promoted out of the old General catch-all.
const GROUPS: { label: string; items: SectionItem[] }[] = [
  {
    label: "Workspace",
    items: [
      { value: "general", label: "General", icon: Settings2, desc: "Updates, notifications, and where recordings are stored." },
      { value: "appearance", label: "Appearance", icon: Palette, desc: "Theme and accent color." },
      { value: "recording", label: "Recording", icon: Mic, desc: "Audio devices and capture preferences." },
      { value: "shortcuts", label: "Shortcuts", icon: Keyboard, desc: "Keyboard shortcuts for recording and navigation." },
    ],
  },
  {
    label: "Intelligence",
    items: [
      { value: "transcription", label: "Transcription", icon: AudioLines, desc: "Speech-to-text engine and model." },
      { value: "summary", label: "Summary", icon: Sparkles, desc: "AI summary provider and model." },
    ],
  },
  {
    label: "Connections",
    items: [
      { value: "integrations", label: "Add-ons", icon: Plug, desc: "Export destinations, calendar context, and meeting automation." },
    ],
  },
  {
    label: "Advanced",
    items: [
      { value: "diagnostics", label: "Diagnostics", icon: Activity, desc: "Build info and troubleshooting." },
      { value: "beta", label: "Beta", icon: FlaskConical, desc: "Experimental features, off by default." },
    ],
  },
];

const ALL_ITEMS = GROUPS.flatMap((g) => g.items);

export default function SettingsPage() {
  const router = useRouter();
  const { transcriptModelConfig, setTranscriptModelConfig } = useConfig();
  const [activeTab, setActiveTab] = useState("general");

  useEffect(() => {
    const loadTranscriptConfig = async () => {
      try {
        const config = (await invoke("api_get_transcript_config")) as any;
        if (config) {
          setTranscriptModelConfig({
            provider: config.provider || "localWhisper",
            model: config.model || "large-v3",
            apiKey: config.apiKey || null,
            baseUrl: config.baseUrl || null,
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
    const requested = new URLSearchParams(window.location.search).get("tab");
    if (requested && ALL_ITEMS.some((i) => i.value === requested)) {
      setActiveTab(requested);
    }
    const onOpenTab = (e: Event) => {
      const tab = (e as CustomEvent<string>).detail;
      if (tab && ALL_ITEMS.some((i) => i.value === tab)) setActiveTab(tab);
    };
    window.addEventListener("open-settings-tab", onOpenTab as EventListener);
    return () => window.removeEventListener("open-settings-tab", onOpenTab as EventListener);
  }, []);

  const active = ALL_ITEMS.find((i) => i.value === activeTab) ?? ALL_ITEMS[0];

  const renderPanel = () => {
    switch (activeTab) {
      case "general":
        return <PreferenceSettings />;
      case "appearance":
        return <ThemeSettings />;
      case "recording":
        return <RecordingSettings />;
      case "shortcuts":
        return <KeyboardShortcutsSettings />;
      case "transcription":
        return (
          <TranscriptSettings
            transcriptModelConfig={transcriptModelConfig}
            setTranscriptModelConfig={setTranscriptModelConfig}
          />
        );
      case "summary":
        return <SummaryModelSettings />;
      case "integrations":
        return <IntegrationsSettings />;
      case "diagnostics":
        return <DiagnosticsSettings />;
      case "beta":
        return <BetaSettings />;
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
          className="flex items-center gap-2 rounded-md px-2.5 py-1.5 text-sm text-muted-foreground transition hover:bg-muted hover:text-foreground"
        >
          <ArrowLeft className="h-4 w-4" />
          Back
        </button>
        <div className="h-5 w-px bg-border" />
        <h1 className="text-lg font-semibold tracking-tight">Settings</h1>
      </header>

      <div className="flex min-h-0 flex-1">
        {/* Left nav rail */}
        <nav className="w-60 shrink-0 overflow-y-auto border-r border-border px-3 py-5">
          {GROUPS.map((group) => (
            <div key={group.label} className="mb-5">
              <p className="px-3 pb-1.5 font-mono text-[11px] uppercase tracking-wider text-muted-foreground/70">
                {group.label}
              </p>
              <div className="space-y-0.5">
                {group.items.map((item) => {
                  const Icon = item.icon;
                  const isActive = item.value === activeTab;
                  return (
                    <button
                      key={item.value}
                      onClick={() => setActiveTab(item.value)}
                      className={cn(
                        "group relative flex w-full items-center gap-2.5 rounded-md px-3 py-1.5 text-sm transition-colors",
                        isActive
                          ? "bg-primary/10 font-medium text-foreground"
                          : "text-muted-foreground hover:bg-muted hover:text-foreground"
                      )}
                    >
                      {isActive && (
                        <motion.span
                          layoutId="settings-active-bar"
                          className="absolute inset-y-1 left-0 w-[3px] rounded-full bg-accent-gradient"
                          transition={{ type: "spring", stiffness: 500, damping: 40 }}
                        />
                      )}
                      <Icon className={cn("h-4 w-4 shrink-0", isActive && "text-primary")} />
                      {item.label}
                    </button>
                  );
                })}
              </div>
            </div>
          ))}
        </nav>

        {/* Content pane */}
        <main className="min-w-0 flex-1 overflow-y-auto">
          <div className="mx-auto w-full max-w-3xl px-8 py-8">
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
