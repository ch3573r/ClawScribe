"use client";

import { SummaryModelSettings } from "@/components/SummaryModelSettings";
import { SummaryLanguageSettings } from "@/components/SummaryLanguageSettings";
import { SummaryTemplateSettings } from "@/components/SummaryTemplateSettings";
import { LiveSharingSetting } from "@/components/LiveTranscriptSharing";
import { SettingsCard, SettingsColumns, SettingsRow } from "@/components/ui/settings-layout";
import { Switch } from "@/components/ui/switch";
import { ProviderDataAside } from "@/components/settings/summary-provider-parts";
import { useConfig } from "@/contexts/ConfigContext";

/**
 * The Summary page: provider and model, summary defaults and live assistance,
 * with what the selected provider receives beside them. Nothing here has a Save
 * button; each control saves as it changes.
 */
export function SummarySettings() {
  const { modelConfig, isAutoSummary, toggleIsAutoSummary } = useConfig();
  return (
    <SettingsColumns aside={<ProviderDataAside provider={modelConfig.provider} ollamaEndpoint={modelConfig.ollamaEndpoint} customEndpoint={modelConfig.customOpenAIEndpoint} />}>
      <SummaryModelSettings />
      <SettingsCard>
        <SettingsRow title="Summarize automatically" htmlFor="summary-auto" description="Generates a summary when a meeting ends">
          <Switch id="summary-auto" checked={isAutoSummary} onCheckedChange={toggleIsAutoSummary} />
        </SettingsRow>
        <SummaryLanguageSettings />
        <SummaryTemplateSettings />
      </SettingsCard>
      <LiveSharingSetting />
    </SettingsColumns>
  );
}
