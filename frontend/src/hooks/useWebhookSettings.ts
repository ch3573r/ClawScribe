"use client";

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Loadable } from "@/lib/privacy";
import { WEBHOOK_CONFIG_CHANGED, WEBHOOK_UPDATED, type WebhookSettingsView } from "@/lib/webhook";

export function useWebhookSettings(includeDeliveries = false): Loadable<WebhookSettingsView> {
  const [state, setState] = useState<Loadable<WebhookSettingsView>>({ status: "loading" });

  useEffect(() => {
    let active = true;
    let request = 0;
    const unlisten: UnlistenFn[] = [];

    async function refresh() {
      const id = ++request;
      try {
        const value = await invoke<WebhookSettingsView>("get_webhook_settings");
        if (active && id === request) setState({ status: "ready", value });
      } catch {
        if (active && id === request) setState({ status: "error" });
      }
    }

    const events = includeDeliveries ? [WEBHOOK_CONFIG_CHANGED, WEBHOOK_UPDATED] : [WEBHOOK_CONFIG_CHANGED];
    for (const event of events) {
      void listen(event, refresh).then(stop => {
        if (active) {
          unlisten.push(stop);
          void refresh();
        } else {
          stop();
        }
      }).catch(() => undefined);
    }
    void refresh();
    window.addEventListener("focus", refresh);
    return () => {
      active = false;
      unlisten.forEach(stop => stop());
      window.removeEventListener("focus", refresh);
    };
  }, [includeDeliveries]);

  return state;
}
