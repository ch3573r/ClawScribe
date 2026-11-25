"use client";

import React, { useState, useEffect, useSyncExternalStore } from "react";
import { ArrowLeft } from "lucide-react";
import { useRouter } from "next/navigation";
import { cn } from "@/lib/utils";
import { About } from "@/components/About";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { filterSettingsGroups, SETTINGS_ITEMS } from "@/components/settings/settings-sections";
import { useConnectionStatus } from "@/components/settings/useConnectionStatus";
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
import { WebhookSettings } from "@/components/settings/WebhookSettings";
import { AssistantAccessSettings } from "@/components/settings/AssistantAccessSettings";
import { DiagnosticsSettings } from "@/components/settings/DiagnosticsSettings";
import { resolveSettingsTab } from "@/components/settings/settings-tabs";
import { StatusDot } from "@/components/ui/settings-layout";
import { useMicrosoftExport } from "@/hooks/useMicrosoftExport";
import { getICalendarServerSnapshot, getICalendarSnapshot, subscribeToICalendars } from "@/services/icalService";

export default function SettingsPage() {
  const router = useRouter();
  const [activeTab, setActiveTab] = useState("general");
  const [sectionQuery, setSectionQuery] = useState("");
  const connections = useConnectionStatus(activeTab);
  const microsoft = useMicrosoftExport();
  const calendars = useSyncExternalStore(subscribeToICalendars, getICalendarSnapshot, getICalendarServerSnapshot);

  // Deep-link support: ?tab=… on load, and the sidebar's open-settings-tab event
  // while already mounted.
  useEffect(() => {
    const requested = resolveSettingsTab(new URLSearchParams(window.location.search).get("tab"));
    if (requested) setActiveTab(requested);
    const onOpenTab = (e: Event) => {
      const tab = resolveSettingsTab((e as CustomEvent<string>).detail);
      if (tab) { setActiveTab(tab); setSectionQuery(""); }
    };
    window.addEventListener("open-settings-tab", onOpenTab as EventListener);
    return () => window.removeEventListener("open-settings-tab", onOpenTab as EventListener);
  }, []);

  const active = SETTINGS_ITEMS.find((i) => i.value === activeTab) ?? SETTINGS_ITEMS[0];

  const groups = filterSettingsGroups(sectionQuery);
  const microsoftConnected = microsoft.connection.state === "connected";
  const microsoftCalendar = microsoftConnected && !microsoft.connection.unavailableExports?.includes("Calendar lookup");
  const calendarIssue = !!calendars.error || Object.keys(calendars.issues).length > 0;
  const dots: typeof connections.dots = {
    ...connections.dots,
    ms365: microsoftConnected ? { tone: "ok", label: "Connected" }
      : microsoft.connection.state === "connecting" ? { tone: "active", label: "Signing in" }
        : microsoft.error || microsoft.connection.state !== "not_connected"
          ? { tone: "warning", label: "Sign-in needs attention" } : { tone: "off", label: "Signed out" },
    calendars: calendarIssue
      ? { tone: "warning", label: "A calendar has a problem" }
      : calendars.sources.length > 0 || microsoftCalendar
        ? { tone: "ok", label: "Calendars connected" }
        : calendars.loading ? { tone: "neutral", label: "Checking calendars" } : { tone: "off", label: "No calendars" },
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
        return <ConfluenceSettings onStatusChange={connections.setConfluence} />;
      case "webhook":
        return <WebhookSettings />;
      case "assistant":
        return <AssistantAccessSettings onStatusChange={connections.setAssistant} />;
      case "about":
        return <About />;
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
        <Button variant="ghost" onClick={() => router.back()}
          className="flex items-center gap-2 rounded-md px-2.5 py-1.5 text-sm text-muted-foreground transition hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          <ArrowLeft className="h-4 w-4" />
          Back
        </Button>
        <div className="h-5 w-px bg-border" />
        <h1 className="text-lg font-semibold tracking-tight">Settings</h1>
      </header>

      <div className="flex min-h-0 flex-1">
        {/* Left nav rail */}
        <nav aria-label="Settings sections" className="w-60 shrink-0 overflow-y-auto border-r border-border px-3 py-5">
          <Input aria-label="Filter settings" placeholder="Filter settings…" value={sectionQuery}
            onChange={event => setSectionQuery(event.target.value)} className="mb-5" />
          {!groups.length && <p role="status" className="px-2 text-sm text-muted-foreground">No matching settings.</p>}
          {groups.map((group) => (
            <div key={group.label} className="mb-6">
              <p className="px-3 pb-2 text-xs text-muted-foreground">{group.label}</p>
              <div className="space-y-0.5">
                {group.items.map((item) => {
                  const Icon = item.icon;
                  const isActive = item.value === active.value;
                  const dot = dots[item.value];
                  return (
                    <Button
                      key={item.value}
                      type="button"
                      variant="ghost"
                      onClick={() => setActiveTab(item.value)}
                      aria-current={isActive ? "page" : undefined}
                      className={cn(
                        "relative h-[34px] w-full justify-start gap-2.5 px-3 text-left text-sm font-normal",
                        isActive
                          ? "bg-primary/10 text-foreground"
                          : "text-muted-foreground hover:bg-muted hover:text-foreground"
                      )}
                    >
                      {isActive && <span aria-hidden="true" className="absolute inset-y-1.5 left-0 w-[3px] rounded-full bg-primary" />}
                      <Icon className={cn("h-4 w-4 shrink-0", isActive && "text-primary")} />
                      <span className="min-w-0 flex-1 truncate">{item.label}</span>
                      {dot && (
                        <Tooltip>
                          <TooltipTrigger asChild><span><StatusDot tone={dot.tone} /><span className="sr-only">, {dot.label}</span></span></TooltipTrigger>
                          <TooltipContent>{dot.label}</TooltipContent>
                        </Tooltip>
                      )}
                    </Button>
                  );
                })}
              </div>
            </div>
          ))}
        </nav>

        {/* Content pane */}
        <main className="min-w-0 flex-1 overflow-y-auto">
          <div className="w-full px-6 pb-14 pt-8 min-[1500px]:px-12">
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
