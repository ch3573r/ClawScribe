"use client";

import React, { useState, useEffect, useLayoutEffect, useRef } from "react";
import {
  ArrowLeft,
  Settings2,
  Mic,
  Database as DatabaseIcon,
  SparkleIcon,
  FlaskConical,
  Plug,
} from "lucide-react";
import { useRouter } from "next/navigation";
import { invoke } from "@tauri-apps/api/core";
import { motion } from "framer-motion";
import { TranscriptSettings } from "@/components/TranscriptSettings";
import { RecordingSettings } from "@/components/RecordingSettings";
import { PreferenceSettings } from "@/components/PreferenceSettings";
import { SummaryModelSettings } from "@/components/SummaryModelSettings";
import { BetaSettings } from "@/components/BetaSettings";
import { IntegrationsSettings } from "@/components/IntegrationsSettings";
import { useConfig } from "@/contexts/ConfigContext";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";

const TABS = [
  { value: "general", label: "General", icon: Settings2 },
  { value: "recording", label: "Recording", icon: Mic },
  { value: "Transcriptionmodels", label: "Transcription", icon: DatabaseIcon },
  { value: "summaryModels", label: "Summary", icon: SparkleIcon },
  { value: "integrations", label: "Add-ons", icon: Plug },
  { value: "beta", label: "Beta", icon: FlaskConical },
] as const;

export default function SettingsPage() {
  const router = useRouter();
  const { transcriptModelConfig, setTranscriptModelConfig } = useConfig();

  const [activeTab, setActiveTab] = useState("general");
  const tabRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const [underlineStyle, setUnderlineStyle] = useState({ left: 0, width: 0 });

  useEffect(() => {
    const loadTranscriptConfig = async () => {
      try {
        const config = (await invoke("api_get_transcript_config")) as any;
        if (config) {
          console.log("Loaded saved transcript config:", config);
          setTranscriptModelConfig({
            provider: config.provider || "localWhisper",
            model: config.model || "large-v3",
            apiKey: config.apiKey || null,
          });
        }
      } catch (error) {
        console.error("Failed to load transcript config:", error);
      }
    };
    loadTranscriptConfig();
  }, [setTranscriptModelConfig]);

  useEffect(() => {
    const requestedTab = new URLSearchParams(window.location.search).get("tab");
    if (requestedTab && TABS.some((tab) => tab.value === requestedTab)) {
      setActiveTab(requestedTab);
    }
    // Also react when the sidebar requests a tab while we're already mounted
    // (query-only navigation doesn't remount this page).
    const onOpenTab = (e: Event) => {
      const tab = (e as CustomEvent<string>).detail;
      if (tab && TABS.some((t) => t.value === tab)) setActiveTab(tab);
    };
    window.addEventListener("open-settings-tab", onOpenTab as EventListener);
    return () =>
      window.removeEventListener("open-settings-tab", onOpenTab as EventListener);
  }, []);

  useLayoutEffect(() => {
    const activeIndex = TABS.findIndex((tab) => tab.value === activeTab);
    const activeTabElement = tabRefs.current[activeIndex];

    if (activeTabElement) {
      const { offsetLeft, offsetWidth } = activeTabElement;
      setUnderlineStyle({ left: offsetLeft, width: offsetWidth });
    }
  }, [activeTab]);

  return (
    <div className="h-screen overflow-hidden bg-background text-foreground">
      <div className="sticky top-0 z-10 border-b border-border bg-background/95 backdrop-blur">
        <div className="mx-auto w-full max-w-[2400px] px-8 py-7">
          <div className="flex items-center justify-between gap-6">
            <div className="flex items-center gap-5">
              <button
                onClick={() => router.back()}
                className="flex items-center gap-2 rounded-md border border-border bg-muted px-3 py-2 text-sm text-muted-foreground transition hover:bg-muted hover:text-foreground"
              >
                <ArrowLeft className="h-4 w-4" />
                Back
              </button>
              <div>
                <h1 className="text-4xl font-semibold tracking-tight text-foreground">
                  Settings
                </h1>
                <p className="mt-2 text-sm text-muted-foreground">
                  Customize how ClawScribe works for you.
                </p>
              </div>
            </div>
          </div>
        </div>
      </div>

      <div className="h-[calc(100vh-121px)] overflow-y-auto">
        <div className="mx-auto w-full max-w-[2400px] p-8 pt-6">
          <Tabs value={activeTab} onValueChange={setActiveTab}>
            <TabsList className="relative h-auto flex-wrap justify-start gap-1 rounded-none border-b border-border bg-transparent p-0">
              {TABS.map((tab, index) => {
                const Icon = tab.icon;
                return (
                  <TabsTrigger
                    key={`${tab.value}-${tab.label}`}
                    value={tab.value}
                    ref={(el) => {
                      tabRefs.current[index] = el;
                    }}
                    className="relative z-10 flex items-center gap-2 rounded-none border-0 bg-transparent px-5 py-4 text-muted-foreground shadow-none transition hover:text-foreground data-[state=active]:bg-transparent data-[state=active]:text-primary data-[state=active]:shadow-none"
                  >
                    <Icon className="h-4 w-4" />
                    {tab.label}
                  </TabsTrigger>
                );
              })}

              <motion.div
                className="absolute bottom-0 z-20 h-0.5 bg-primary shadow-[0_0_18px_hsl(var(--primary)/0.6)]"
                layoutId="underline"
                style={{
                  left: underlineStyle.left,
                  width: underlineStyle.width,
                }}
                transition={{ type: "spring", stiffness: 400, damping: 40 }}
              />
            </TabsList>

            <TabsContent value="general" className="mt-6">
              <PreferenceSettings />
            </TabsContent>
            <TabsContent value="recording" className="mt-6">
              <RecordingSettings />
            </TabsContent>
            <TabsContent value="Transcriptionmodels" className="mt-6">
              <TranscriptSettings
                transcriptModelConfig={transcriptModelConfig}
                setTranscriptModelConfig={setTranscriptModelConfig}
              />
            </TabsContent>
            <TabsContent value="summaryModels" className="mt-6">
              <SummaryModelSettings />
            </TabsContent>
            <TabsContent value="integrations" className="mt-6">
              <IntegrationsSettings />
            </TabsContent>
            <TabsContent value="beta" className="mt-6">
              <BetaSettings />
            </TabsContent>
          </Tabs>
        </div>
      </div>
    </div>
  );
}
