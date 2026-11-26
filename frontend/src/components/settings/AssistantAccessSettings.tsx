"use client";

import { LocalMcpSettings } from "@/components/LocalMcpSettings";
import { SettingsStack } from "@/components/ui/settings-layout";

export function AssistantAccessSettings({ onStatusChange }: Parameters<typeof LocalMcpSettings>[0] = {}) {
  return <SettingsStack><LocalMcpSettings onStatusChange={onStatusChange} /></SettingsStack>;
}
