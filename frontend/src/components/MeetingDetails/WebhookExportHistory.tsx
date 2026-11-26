"use client";

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Button } from "@/components/ui/button";
import { WEBHOOK_UPDATED, webhookDeliveryMessage, type WebhookDelivery } from "@/lib/webhook";

const eventLabels: Record<WebhookDelivery["event"], string> = {
  "summary.ready": "Summary ready",
  "recording.saved": "Recording saved",
  test: "Test delivery",
};

export function WebhookExportHistory({ meetingId }: { meetingId: string }) {
  const [deliveries, setDeliveries] = useState<WebhookDelivery[] | null>(null);
  const [hasError, setHasError] = useState(false);
  const [retry, setRetry] = useState(0);

  useEffect(() => {
    let active = true;
    let request = 0;
    async function refresh() {
      const version = ++request;
      try {
        const rows = await invoke<WebhookDelivery[]>("get_meeting_webhook_history", { meetingId });
        if (!active || version !== request) return;
        setDeliveries(rows);
        setHasError(false);
      } catch {
        if (active && version === request) setHasError(true);
      }
    }
    void refresh();
    const subscription = listen(WEBHOOK_UPDATED, refresh);
    void subscription.then(() => { if (active) void refresh(); }).catch(() => undefined);
    return () => {
      active = false;
      void subscription.then(stop => stop()).catch(() => undefined);
    };
  }, [meetingId, retry]);

  return <section aria-label="Webhook deliveries" className="space-y-2 border-t border-border pt-3">
    <h3 className="text-sm font-medium">Webhook deliveries</h3>
    {hasError && <div className="space-y-2">
      <p role="alert" className="text-sm text-destructive">Could not load webhook deliveries.</p>
      <Button type="button" variant="outline" size="sm" onClick={() => {
        setHasError(false);
        setRetry(value => value + 1);
      }}>Retry loading webhook deliveries</Button>
    </div>}
    {!hasError && deliveries === null && <p role="status" className="text-sm text-muted-foreground">Loading webhook deliveries…</p>}
    {!hasError && deliveries?.length === 0 && <p className="text-sm text-muted-foreground">No webhook deliveries recorded.</p>}
    {!!deliveries?.length && <ul className="divide-y divide-border">
      {deliveries.map(delivery => <li key={delivery.id} className="space-y-1 py-2 text-sm">
        <p>{eventLabels[delivery.event]}</p>
        <p className="text-xs text-muted-foreground">{webhookDeliveryMessage(delivery)}</p>
        <p className="text-xs text-muted-foreground">
          <time dateTime={delivery.startedAt}>{new Date(delivery.startedAt).toLocaleString()}</time>
          {delivery.attempts > 0 && ` · ${delivery.attempts} ${delivery.attempts === 1 ? "attempt" : "attempts"}`}
        </p>
      </li>)}
    </ul>}
  </section>;
}
