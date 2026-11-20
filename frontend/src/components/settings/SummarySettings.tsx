"use client";

import { useRef, useState } from "react";
import { SummaryModelSettings } from "@/components/SummaryModelSettings";
import { SummaryLanguageSettings } from "@/components/SummaryLanguageSettings";
import { SummaryTemplateSettings } from "@/components/SummaryTemplateSettings";
import { LiveSharingSetting } from "@/components/LiveTranscriptSharing";
import { SettingsCard, SettingsColumns, SettingsRow } from "@/components/ui/settings-layout";
import { Switch } from "@/components/ui/switch";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { ProviderDataAside } from "@/components/settings/summary-provider-parts";
import { providerLabel } from "@/components/settings/provider-data";
import { useConfig } from "@/contexts/ConfigContext";
import { useSummaryDestination, type SummaryDestination } from "@/hooks/useSummaryDestination";

/**
 * The Summary page: provider and model, summary defaults and live assistance,
 * with what the selected provider receives beside them. Nothing here has a Save
 * button; each control saves as it changes.
 */
export function SummarySettings() {
  const { modelConfig, isAutoSummary, automaticSummaryBusy, toggleIsAutoSummary } = useConfig();
  const { destination, error, refresh } = useSummaryDestination(true, modelConfig, isAutoSummary);
  const [review, setReview] = useState<SummaryDestination | null>(null);
  const autoSwitch = useRef<HTMLButtonElement>(null);

  async function changeAutomaticSummary(enabled: boolean) {
    if (enabled && destination?.external) {
      setReview(destination);
    } else if (!enabled || destination) {
      await toggleIsAutoSummary(enabled, destination?.fingerprint);
    }
  }

  async function approveDestination() {
    if (!review) return;
    if (await toggleIsAutoSummary(true, review.fingerprint)) setReview(null);
    refresh();
  }

  const reviewRequired = isAutoSummary && destination?.external && !destination.approved;
  const reviewIsCurrent = review !== null && destination?.fingerprint === review.fingerprint;
  return (
    <SettingsColumns aside={<ProviderDataAside provider={modelConfig.provider} ollamaEndpoint={modelConfig.ollamaEndpoint} customEndpoint={modelConfig.customOpenAIEndpoint} />}>
      <SummaryModelSettings />
      <SettingsCard>
        <SettingsRow title="Summarize automatically" htmlFor="summary-auto" description={error ?? (
          reviewRequired ? "Automatic notes are paused until you review the new destination."
            : "Generate notes with your selected summary provider after a recording is saved."
        )}>
          {reviewRequired && <Button variant="outline" size="sm" disabled={automaticSummaryBusy} onClick={() => setReview(destination)}>Review destination</Button>}
          <Switch ref={autoSwitch} id="summary-auto" checked={isAutoSummary} disabled={automaticSummaryBusy || (!isAutoSummary && !destination)} onCheckedChange={changeAutomaticSummary} />
        </SettingsRow>
        <SummaryLanguageSettings />
        <SummaryTemplateSettings />
      </SettingsCard>
      <LiveSharingSetting />
      <Dialog open={review !== null} onOpenChange={open => { if (!open) setReview(null); }}>
        <DialogContent onCloseAutoFocus={event => {
          event.preventDefault();
          autoSwitch.current?.focus();
        }}>
          <DialogHeader>
            <DialogTitle>Allow automatic notes with {review && providerLabel(review.provider)}?</DialogTitle>
            <DialogDescription>After you save a recording, ClawScribe will send its transcript and notes context to the destination below to generate notes.</DialogDescription>
          </DialogHeader>
          {review && <div className="space-y-1 text-sm">
            <p className="break-all">{review.endpoint}</p>
            <p className="break-all text-muted-foreground">{review.model}</p>
            {review.unencrypted && <p className="text-warning-foreground">This connection uses unencrypted HTTP.</p>}
            {!reviewIsCurrent && <p role="status">Close this review and check the current destination before allowing automatic notes.</p>}
          </div>}
          <DialogFooter>
            <Button variant="outline" onClick={() => setReview(null)}>Cancel</Button>
            <Button disabled={automaticSummaryBusy || !reviewIsCurrent} onClick={approveDestination}>Allow automatic notes</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </SettingsColumns>
  );
}
