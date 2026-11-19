"use client";

import { LocalMcpSettings } from "@/components/LocalMcpSettings";
import { SettingsStack } from "@/components/ui/settings-layout";

export function AssistantAccessSettings() {
  return <SettingsStack><LocalMcpSettings /></SettingsStack>;
}
