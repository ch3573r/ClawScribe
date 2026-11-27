"use client";

import { invoke } from "@tauri-apps/api/core";
import { useEffect, useId, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { SettingsCard, SettingsRow } from "@/components/ui/settings-layout";
import { Switch } from "@/components/ui/switch";

type Preferences = { enabled: boolean; autoApply: boolean };
type Profile = { key: string; name: string };
type Settings = { preferences: Preferences; profiles: Profile[] };

export function SpeakerProfilesSettings() {
  const id = useId();
  const [settings, setSettings] = useState<Settings>();
  const [attempt, setAttempt] = useState(0);
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const [error, setError] = useState("");
  const [selected, setSelected] = useState<Profile>();

  useEffect(() => {
    let active = true;
    void invoke<Settings>("get_speaker_profile_settings").then(
      result => { if (active) setSettings(result); },
      () => { if (active) setError("Could not load speaker profiles."); },
    );
    return () => { active = false; };
  }, [attempt]);

  async function save(operation: () => Promise<void>, failure: string) {
    if (pending.current) return;
    pending.current = true;
    setBusy(true);
    setError("");
    try {
      await operation();
    } catch {
      setError(failure);
    } finally {
      pending.current = false;
      setBusy(false);
    }
  }

  function setPreferences(enabled: boolean, autoApply: boolean) {
    return save(async () => {
      const preferences = await invoke<Preferences>("set_speaker_profile_preferences", { enabled, autoApply });
      setSettings(current => current && { ...current, preferences });
    }, "Could not update speaker profile settings. Please try again.");
  }

  function deleteProfile(profile: Profile) {
    return save(async () => {
      await invoke("delete_speaker_profile", { key: profile.key });
      setSettings(current => current && { ...current, profiles: current.profiles.filter(item => item.key !== profile.key) });
      setSelected(undefined);
    }, "Could not delete the speaker profile. Please try again.");
  }

  return (
    <SettingsCard title="Speaker profiles">
      {!settings ? (
        error ? (
          <SettingsRow title={error} role="alert">
            <Button type="button" variant="outline" size="sm" onClick={() => { setError(""); setAttempt(value => value + 1); }}>Try again</Button>
          </SettingsRow>
        ) : <SettingsRow title="Loading speaker profiles…" role="status" />
      ) : (
        <>
          <SettingsRow title="Use speaker profiles" htmlFor={id} description={
            <span id={`${id}-consent`}>When you choose to remember a voice, ClawScribe stores its voice characteristics on this device to suggest names in future meetings. Voice data stays on this device and is excluded from library backups.</span>
          }>
            <Switch id={id} aria-describedby={`${id}-consent`} checked={settings.preferences.enabled} disabled={busy}
              onCheckedChange={enabled => void setPreferences(enabled, false)} />
          </SettingsRow>
          {settings.preferences.enabled && (
            <SettingsRow title="Apply matched names automatically" htmlFor={`${id}-auto`}
              description={<span id={`${id}-matching`}>Apply strong matches when you run Detect speakers. You can review and change the labels in the transcript.</span>}>
              <Switch id={`${id}-auto`} aria-describedby={`${id}-matching`} checked={settings.preferences.autoApply} disabled={busy} onCheckedChange={value => void setPreferences(true, value)} />
            </SettingsRow>
          )}
          {settings.profiles.length ? settings.profiles.map(profile => (
            <SettingsRow key={profile.key} title={<span className="break-words">{profile.name}</span>}>
              <Button type="button" aria-label={`Delete the profile for ${profile.name}`} variant="outline" size="sm" disabled={busy}
                onClick={() => { setError(""); setSelected(profile); }}>Delete</Button>
            </SettingsRow>
          )) : settings.preferences.enabled && (
            <SettingsRow title="No saved voices" description="Run Detect speakers in a saved meeting, then choose to remember a voice when you rename it." />
          )}
          {error && !selected && <SettingsRow title={error} role="alert" />}
          <Dialog open={Boolean(selected)} onOpenChange={open => { if (!open && !busy) { setSelected(undefined); setError(""); } }}>
            <DialogContent onEscapeKeyDown={event => { if (busy) event.preventDefault(); }}>
              <DialogHeader>
                <DialogTitle>Delete speaker profile?</DialogTitle>
                <DialogDescription>This removes the saved voice data for {selected?.name}. Edit applied names directly in the transcript.</DialogDescription>
              </DialogHeader>
              {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
              <DialogFooter>
                <Button type="button" variant="outline" disabled={busy} onClick={() => { setSelected(undefined); setError(""); }}>Cancel</Button>
                <Button type="button" variant="destructive" disabled={busy || !selected} onClick={() => { if (selected) void deleteProfile(selected); }}>
                  {busy ? "Deleting…" : "Delete profile"}
                </Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>
        </>
      )}
    </SettingsCard>
  );
}
