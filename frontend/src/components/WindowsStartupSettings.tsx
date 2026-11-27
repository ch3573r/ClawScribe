"use client";

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { disable, isEnabled } from "@tauri-apps/plugin-autostart";
import { usePlatform } from "@/hooks/usePlatform";
import { Button } from "./ui/button";
import { SettingsCard, SettingsRow } from "./ui/settings-layout";
import { Switch } from "./ui/switch";

export function WindowsStartupSettings() {
  const platform = usePlatform();
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [readAttempt, setReadAttempt] = useState(0);
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const pending = useRef(false);

  useEffect(() => {
    if (platform !== "windows") return;
    let active = true;
    void isEnabled().then(value => {
      if (active) setEnabled(value);
    }).catch(() => {
      if (active) setError("Could not read the Windows startup setting.");
    });
    return () => { active = false; };
  }, [platform, readAttempt]);

  if (platform !== "windows") return null;

  const change = async (next: boolean) => {
    if (enabled === null || enabled === next || pending.current) return;
    pending.current = true;
    setSaving(true);
    setError("");
    try {
      await (next ? invoke("enable_windows_autostart") : disable());
      setEnabled(next);
    } catch {
      await isEnabled().then(setEnabled, () => {});
      setError("Could not change the Windows startup setting. Try again.");
    } finally {
      pending.current = false;
      setSaving(false);
    }
  };

  return (
    <SettingsCard title="Startup">
      <SettingsRow
        title="Start ClawScribe when I sign in to Windows"
        htmlFor="preferences-windows-autostart"
        description="Starts in the system tray so call detection can run when enabled."
      >
        {(saving || (enabled === null && !error)) && (
          <span role="status" className="text-sm text-muted-foreground">{saving ? "Saving…" : "Loading…"}</span>
        )}
        <Switch id="preferences-windows-autostart" checked={enabled ?? false} disabled={enabled === null || saving} onCheckedChange={change} />
      </SettingsRow>
      {error && (
        <SettingsRow title={<span role="alert" className="text-destructive">{error}</span>}>
          {enabled === null && (
            <Button type="button" variant="outline" size="sm" onClick={() => {
              setError("");
              setReadAttempt(attempt => attempt + 1);
            }}>Retry</Button>
          )}
        </SettingsRow>
      )}
    </SettingsCard>
  );
}
