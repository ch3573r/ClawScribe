"use client";

import { useEffect, useState } from "react";
import { toast } from "sonner";
import { TeamsIcon } from "@/components/IntegrationIcons";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Meta, SettingsCard, SettingsFooter, SettingsRow, StatusText } from "@/components/ui/settings-layout";
import { ROW_SELECT } from "@/components/settings/settings-parts";
import { clockTime, describeCallCheck, useCallDetectionCheck } from "@/components/settings/call-detection-check";
import {
  CALL_PROVIDERS, getCallSettings, setCallSettings,
  type CallProvider, type CallSettings, type TeamsDetectionMode,
} from "@/lib/autoRecord";

/** How ClawScribe reacts when a call is detected, and a manual check of the same scan. */
export function CallDetectionSettings() {
  const [settings, update] = useState<CallSettings>({ mode: "off", providers: ["teams"], autoStop: false });
  const { state, check, reset } = useCallDetectionCheck(settings.providers);

  useEffect(() => {
    update(getCallSettings());
  }, []);

  const change = (next: CallSettings) => {
    try {
      setCallSettings(next);
      update(next);
      reset();
    } catch { toast.error("Could not save call detection settings."); }
  };

  const result = describeCallCheck(state);
  return (
    <SettingsCard title="Call detection" description="Notices an active call in the apps you choose">
      <SettingsRow title="When a call is detected" htmlFor="call-detection-mode">
        <Select value={settings.mode} onValueChange={(value) => change({ ...settings, mode: value as TeamsDetectionMode })}>
          <SelectTrigger id="call-detection-mode" aria-label="When a call is detected" className={ROW_SELECT}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="off">Do nothing</SelectItem>
            <SelectItem value="prompt">Prompt me to record</SelectItem>
            <SelectItem value="auto">Auto-start recording</SelectItem>
          </SelectContent>
        </Select>
      </SettingsRow>
      <SettingsRow title="Meeting apps">
        <fieldset aria-label="Meeting apps" className="flex flex-wrap items-center justify-end gap-x-6 gap-y-2">
          {(Object.keys(CALL_PROVIDERS) as CallProvider[]).map(provider => (
            <label key={provider} className="flex cursor-pointer items-center gap-2 text-sm text-foreground">
              <Checkbox checked={settings.providers.includes(provider)} onCheckedChange={on => change({
                ...settings, providers: on === true ? [...settings.providers, provider] : settings.providers.filter(p => p !== provider),
              })} />
              {provider === "teams" && <TeamsIcon aria-hidden="true" className="h-4 w-4" />}
              {CALL_PROVIDERS[provider]}
            </label>
          ))}
        </fieldset>
      </SettingsRow>
      <SettingsRow
        title="Stop and save when the call ends"
        htmlFor="call-detection-auto-stop"
        description="Only for recordings that call detection started. Switching browser tabs keeps the recording running."
      >
        <Switch id="call-detection-auto-stop" checked={settings.autoStop} disabled={settings.mode === "off" || !settings.providers.length}
          onCheckedChange={autoStop => change({ ...settings, autoStop })} />
      </SettingsRow>
      <SettingsFooter
        actions={
          <Button type="button" variant="ghost" size="sm" disabled={state.checking || !settings.providers.length} onClick={() => void check()}>
            {state.checking ? "Checking…" : "Check now"}
          </Button>
        }
      >
        <span role="status"><StatusText tone={result.tone}>{result.text}</StatusText></span>
        {state.checkedAt && <Meta>Checked {clockTime(state.checkedAt)}</Meta>}
        {!settings.providers.length && <span>Select at least one app to check</span>}
      </SettingsFooter>
    </SettingsCard>
  );
}
