"use client";

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useConfig } from "@/contexts/ConfigContext";
import { useCloudTranscription } from "@/hooks/useCloudTranscription";
import { getExportDestinations } from "@/lib/exportDestinations";
import type { Loadable, PrivacyInput } from "@/lib/privacy";
import { AUTO_UPDATE_CHECK_CHANGED_EVENT, getAutoUpdateCheckEnabled } from "@/lib/updatePreferences";
import { knowledgeService } from "@/services/knowledgeService";
import { microsoftExportService } from "@/services/microsoftExportService";

/**
 * Reads the settings the Privacy page reports on. Every read is read-only and
 * goes through the same commands and services as the page that edits the
 * setting. Values are refreshed when the page is shown again.
 */
function useLoadable<T>(load: () => Promise<T>, tick: number): Loadable<T> {
  const [state, setState] = useState<Loadable<T>>({ status: "loading" });
  const loadRef = useRef(load);
  loadRef.current = load;
  useEffect(() => {
    let cancelled = false;
    loadRef.current()
      .then((value) => { if (!cancelled) setState({ status: "ready", value }); })
      .catch(() => { if (!cancelled) setState({ status: "error" }); });
    return () => { cancelled = true; };
  }, [tick]);
  return state;
}

export function usePrivacyInput(): PrivacyInput {
  const { isAutoSummary } = useConfig();
  const cloud = useCloudTranscription();
  const [tick, setTick] = useState(0);
  const [updateCheck, setUpdateCheck] = useState(true);
  const [confluenceRest, setConfluenceRest] = useState(false);

  // Refresh when the window or tab comes back into view.
  useEffect(() => {
    const sync = () => {
      setUpdateCheck(getAutoUpdateCheckEnabled());
      const destinations = getExportDestinations();
      setConfluenceRest(destinations.confluenceMode === "rest" && !!destinations.confluenceBaseUrl?.trim());
    };
    const refresh = () => {
      setTick((value) => value + 1);
      sync();
    };
    const onVisibility = () => { if (document.visibilityState === "visible") refresh(); };
    sync();
    window.addEventListener("focus", refresh);
    window.addEventListener("storage", refresh);
    window.addEventListener(AUTO_UPDATE_CHECK_CHANGED_EVENT, refresh);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.removeEventListener("focus", refresh);
      window.removeEventListener("storage", refresh);
      window.removeEventListener(AUTO_UPDATE_CHECK_CHANGED_EVENT, refresh);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, []);

  const recording = useLoadable(async () => {
    const preferences = await invoke<{ save_folder?: string; auto_save?: boolean }>("get_recording_preferences");
    return { saveFolder: preferences?.save_folder ?? "", saveAudio: preferences?.auto_save !== false };
  }, tick);

  const transcription = useLoadable(async () => {
    const config = await invoke<{ provider?: string; baseUrl?: string | null } | null>("api_get_transcript_config");
    return { provider: config?.provider || "parakeet", baseUrl: config?.baseUrl ?? null };
  }, tick);

  // Only the provider and its endpoint are kept. API keys in these responses are dropped here.
  const summary = useLoadable(async () => {
    const config = await invoke<{ provider?: string | null; ollamaEndpoint?: string | null } | null>("api_get_model_config");
    const provider = config?.provider ?? null;
    let customEndpoint: string | null = null;
    if (provider === "custom-openai") {
      const custom = await invoke<{ endpoint?: string | null } | null>("api_get_custom_openai_config");
      customEndpoint = custom?.endpoint ?? null;
    }
    return { provider, ollamaEndpoint: config?.ollamaEndpoint ?? null, customEndpoint };
  }, tick);

  const liveSharing = useLoadable(() => knowledgeService.liveSharing(), tick);

  const microsoft = useLoadable(async () => {
    const status = await microsoftExportService.connectionStatus();
    const connected = status.state === "connected";
    return { connected, calendar: connected && !status.unavailableExports?.includes("Calendar lookup") };
  }, tick);

  const calendars = useLoadable(async () => {
    const sources = await invoke<{ kind: "file" | "subscription" }[]>("list_ical_sources");
    return {
      files: sources.filter((source) => source.kind === "file").length,
      subscriptions: sources.filter((source) => source.kind === "subscription").length,
    };
  }, tick);

  const assistant = useLoadable(async () => (await invoke<{ enabled: boolean }>("get_mcp_access")).enabled, tick);
  const semanticSearch = useLoadable(async () => (await knowledgeService.modelStatus()).enabled, tick);
  // Only whether handoff is on and where it points; the token never leaves the backend.
  const handoff = useLoadable(async () => {
    const status = await invoke<{ enabled?: boolean; endpoint?: string | null }>("get_openclaw_config_status");
    return { enabled: status?.enabled === true, endpoint: status?.endpoint ?? null };
  }, tick);

  return {
    recording,
    transcription: cloud.loaded ? transcription : { status: "loading" },
    cloudTranscription: cloud.enabled,
    summary,
    autoSummary: isAutoSummary,
    liveSharing,
    microsoft,
    confluenceRest,
    calendars,
    assistant,
    updateCheck,
    semanticSearch,
    handoff,
  };
}
