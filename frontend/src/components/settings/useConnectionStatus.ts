"use client";

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useWebhookSettings } from "@/hooks/useWebhookSettings";
import { canSendWebhook } from "@/lib/webhook";
import { getExportDestinations, hasConfluenceDraftTarget } from "@/lib/exportDestinations";
import { confluenceExportService, type ConfluenceConnectionStatus } from "@/services/confluenceExportService";
import type { StatusTone } from "@/components/ui/settings-layout";

type NavDot = { tone: StatusTone; label: string };
type AssistantAccess = { enabled: boolean; follow_through: boolean };
const checking: NavDot = { tone: "neutral", label: "Checking status" };
const unavailable: NavDot = { tone: "warning", label: "Status unavailable" };

export function useConnectionStatus(activeTab: string) {
  const webhook = useWebhookSettings(true);
  const [confluence, setConfluence] = useState<NavDot>(checking);
  const [assistant, setAssistant] = useState<AssistantAccess | null | undefined>(undefined);

  useEffect(() => {
    let active = true;
    let revision = 0;
    const refresh = () => {
      const request = ++revision;
      if (activeTab !== "confluence") {
        const saved = getExportDestinations();
        if (saved.confluenceMode !== "rest") {
          setConfluence(confluenceDot(saved.confluenceMode, hasConfluenceDraftTarget(saved), null));
        } else {
          void confluenceExportService.settingsStatus(saved.confluenceBaseUrl ?? "").then(
            status => { if (active && revision === request) setConfluence(confluenceDot(saved.confluenceMode, false, status)); },
            () => { if (active && revision === request) setConfluence(unavailable); },
          );
        }
      }
      if (activeTab !== "assistant") {
        void invoke<AssistantAccess>("get_mcp_access").then(
          access => { if (active && revision === request) setAssistant(access); },
          () => { if (active && revision === request) setAssistant(null); },
        );
      }
    };
    refresh();
    window.addEventListener("focus", refresh);
    return () => { active = false; window.removeEventListener("focus", refresh); };
  }, [activeTab]);

  const dots: Record<string, NavDot> = {
    confluence,
    webhook: webhook.status === "loading" ? checking : webhook.status === "error" ? unavailable
      : webhook.value.configurationError ? { tone: "warning", label: "Needs attention" }
        : !canSendWebhook(webhook.value) ? { tone: "off", label: "Off" }
          : webhook.value.lastDelivery?.outcome === "failed" ? { tone: "warning", label: "Last delivery failed" }
            : { tone: "ok", label: "Enabled" },
    assistant: assistant === undefined ? checking : assistant === null ? unavailable
      : assistant.enabled || assistant.follow_through ? { tone: "ok", label: "Access enabled" }
        : { tone: "off", label: "Access off" },
  };
  return { dots, setConfluence, setAssistant };
}

export function confluenceDot(mode: string | undefined, hasDraft: boolean, status: ConfluenceConnectionStatus | null, failed = false): NavDot {
  if (mode !== "rest") return hasDraft ? { tone: "ok", label: "Browser draft configured" } : { tone: "off", label: "No destination" };
  if (!status) return checking;
  if (failed || status.destinationProblem) return { tone: "warning", label: "Needs attention" };
  if (status.reachable) return { tone: "ok", label: "Connected" };
  return status.tokenConfigured ? { tone: "neutral", label: "Token saved" } : { tone: "off", label: "No token saved" };
}
