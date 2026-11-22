"use client";

import { useRef, useState, type FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { UnencryptedHttpOptIn } from "@/components/UnencryptedHttpOptIn";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { SettingsCard, SettingsFooter, SettingsRow, SettingsStack } from "@/components/ui/settings-layout";
import { Switch } from "@/components/ui/switch";
import { useWebhookSettings } from "@/hooks/useWebhookSettings";
import { secretDestinationError } from "@/lib/secretDestination";
import {
  canSendWebhook,
  webhookCommandError,
  webhookDeliveryMessage,
  type WebhookDelivery,
  type WebhookSettingsView,
  type WebhookSwitches,
} from "@/lib/webhook";

const EVENT_LABELS = {
  test: "Test",
  "summary.ready": "Summary ready",
  "recording.saved": "Recording saved",
};

export function WebhookSettings() {
  const state = useWebhookSettings(true);
  return (
    <SettingsStack>
      {state.status === "ready"
        ? <WebhookForm key={state.value.revision ?? "default"} view={state.value} />
        : <SettingsCard>
            <SettingsFooter>
              {state.status === "loading"
                ? <span role="status">Loading webhook settings…</span>
                : <span role="alert" className="text-destructive">Could not load webhook settings.</span>}
            </SettingsFooter>
          </SettingsCard>}
    </SettingsStack>
  );
}

function WebhookForm({ view }: { view: WebhookSettingsView }) {
  const [switches, setSwitches] = useState<WebhookSwitches>({
    enabled: view.enabled,
    onSummary: view.onSummary,
    onRecording: view.onRecording,
    includeSummary: view.includeSummary,
    includeTranscript: view.includeTranscript,
    allowUnencrypted: view.allowUnencrypted,
  });
  const [url, setUrl] = useState("");
  const [secret, setSecret] = useState("");
  const [removeSecret, setRemoveSecret] = useState(false);
  const pending = useRef(false);
  const [busy, setBusy] = useState<"save" | "remove" | "test" | null>(null);
  const [error, setError] = useState("");
  const edited = !!url.trim() || !!secret || removeSecret
    || (Object.keys(switches) as (keyof WebhookSwitches)[]).some(key => switches[key] !== view[key]);
  const endpoint = url.trim() || view.destination || "";
  const invalidDestination = secretDestinationError(endpoint, switches.allowUnencrypted, true);
  const saveDisabled = !!busy || !edited || !!invalidDestination
    || (switches.enabled && !endpoint) || (!!secret && !endpoint);

  function change<K extends keyof WebhookSwitches>(key: K, value: WebhookSwitches[K]) {
    setSwitches(current => ({ ...current, [key]: value }));
  }

  async function run(operation: NonNullable<typeof busy>, action: () => Promise<void>) {
    if (pending.current) return;
    pending.current = true;
    setBusy(operation);
    setError("");
    try {
      await action();
    } catch (failure) {
      setError(webhookCommandError(failure, "Could not complete the webhook request."));
    } finally {
      pending.current = false;
      setBusy(null);
    }
  }

  function save(event: FormEvent) {
    event.preventDefault();
    if (saveDisabled) return;
    void run("save", async () => {
      await invoke("save_webhook_settings", {
        changes: {
          ...switches,
          revision: view.revision,
          url: url.trim() || null,
          signingSecret: secret || null,
          removeSigningSecret: removeSecret,
        },
      });
      setUrl("");
      setSecret("");
      toast.success("Webhook settings saved");
    });
  }

  function sendTest() {
    if (!canSendWebhook(view) || edited) return;
    void run("test", async () => {
      const delivery = await invoke<WebhookDelivery>("send_webhook_test", { revision: view.revision });
      if (delivery.outcome === "succeeded") toast.success("Test webhook delivered");
      else setError(webhookDeliveryMessage(delivery));
    });
  }

  function remove() {
    void run("remove", async () => {
      await invoke("remove_webhook_settings", { revision: view.revision });
      setUrl("");
      setSecret("");
      toast.success("Webhook removed");
    });
  }

  const delivery = view.lastDelivery;
  return (
    <form onSubmit={save} className="grid min-w-0 gap-5">
      <SettingsCard
        title="Destination"
      >
        <SettingsRow title="Enable webhook" htmlFor="webhook-enabled"
          description="Allow ClawScribe to send meeting data to the saved destination.">
          <Switch id="webhook-enabled" checked={switches.enabled} disabled={!!busy} onCheckedChange={value => change("enabled", value)} />
        </SettingsRow>
        <div className="grid gap-2 px-5 py-4">
          <label className="text-sm" htmlFor="webhook-url">Destination URL</label>
          {view.destination
            ? <p className="break-all text-sm text-muted-foreground">Saved destination {view.destination}/••••</p>
            : <p className="text-sm text-muted-foreground">Enter the URL provided by your automation service.</p>}
          <Input id="webhook-url" type="password" value={url} placeholder={view.configured ? "Enter a replacement URL" : "https://hooks.example.com/…"}
            autoComplete="new-password" disabled={!!busy} onChange={event => {
              setUrl(event.target.value);
              change("allowUnencrypted", event.target.value.trim() ? false : view.allowUnencrypted);
            }} />
          <UnencryptedHttpOptIn urls={[endpoint]} checked={switches.allowUnencrypted}
            onChange={value => change("allowUnencrypted", value)} includeLoopback disabled={!!busy}
            description="HTTP sends the URL and meeting data without encryption." />
          {view.configurationError && !url.trim() &&
            <p role="alert" className="text-sm text-destructive">{view.configurationError}</p>}
        </div>
        <div className="grid gap-2 px-5 py-4">
          <label className="text-sm" htmlFor="webhook-secret">Signing secret (optional)</label>
          <Input id="webhook-secret" type="password" value={secret} autoComplete="new-password"
            disabled={!!busy || removeSecret} onChange={event => setSecret(event.target.value)} />
          {view.signingSecret.stored && <>
            <p className="text-sm text-muted-foreground">
              {view.signingSecret.hint ? `Saved secret ending in ${view.signingSecret.hint}.` : "Signing secret saved."} Changing the URL clears the secret unless you enter a new one.
            </p>
            <label className="flex items-center gap-2 text-sm">
              <Checkbox checked={removeSecret} disabled={!!busy} onCheckedChange={value => {
                setRemoveSecret(value === true);
                if (value === true) setSecret("");
              }} />
              Remove saved signing secret
            </label>
          </>}
        </div>
      </SettingsCard>

      <SettingsCard title="When to send" description="You can also send a meeting from its export menu.">
        <SettingsRow title="When a summary is ready" htmlFor="webhook-summary">
          <Switch id="webhook-summary" checked={switches.onSummary} disabled={!!busy} onCheckedChange={value => change("onSummary", value)} />
        </SettingsRow>
        <SettingsRow title="When a recording is saved" description="Includes completed audio imports." htmlFor="webhook-recording">
          <Switch id="webhook-recording" checked={switches.onRecording} disabled={!!busy} onCheckedChange={value => change("onRecording", value)} />
        </SettingsRow>
      </SettingsCard>

      <SettingsCard title="What to send" description="Meeting details include the title, date, duration, tags and confirmed attendees when available.">
        <SettingsRow title="Include summary and action items" htmlFor="webhook-include-summary">
          <Switch id="webhook-include-summary" checked={switches.includeSummary} disabled={!!busy} onCheckedChange={value => change("includeSummary", value)} />
        </SettingsRow>
        <SettingsRow title="Include transcript" description="Sends saved transcript text with speaker names. Audio stays on this PC." htmlFor="webhook-include-transcript">
          <Switch id="webhook-include-transcript" checked={switches.includeTranscript} disabled={!!busy} onCheckedChange={value => change("includeTranscript", value)} />
        </SettingsRow>
        <SettingsFooter actions={
          <>
            {view.configured && <Button type="button" variant="ghost" size="sm" disabled={!!busy} onClick={remove}>Remove</Button>}
            {view.configured && <Button type="button" variant="outline" size="sm" disabled={!!busy || edited || !canSendWebhook(view)} onClick={sendTest}>
              {busy === "test" ? "Sending test…" : "Send test"}
            </Button>}
            <Button type="submit" size="sm" disabled={saveDisabled}>{busy === "save" ? "Saving…" : "Save"}</Button>
          </>
        }>
          {error && <span role="alert" className="text-destructive">{error}</span>}
        </SettingsFooter>
      </SettingsCard>

      <SettingsCard title="Last delivery">
        <SettingsFooter>
          <span role="status" className={delivery?.outcome === "failed" ? "text-destructive" : undefined}>
            {delivery ? (
              <>
                {EVENT_LABELS[delivery.event]} · {new Date(delivery.finishedAt ?? delivery.startedAt).toLocaleString()} · {webhookDeliveryMessage(delivery)}
                {delivery.outcome !== "pending" && ` · ${delivery.attempts} ${delivery.attempts === 1 ? "attempt" : "attempts"}`}
              </>
            ) : "No deliveries yet"}
          </span>
        </SettingsFooter>
      </SettingsCard>
    </form>
  );
}
