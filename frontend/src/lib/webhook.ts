export interface WebhookSwitches {
  enabled: boolean;
  onSummary: boolean;
  onRecording: boolean;
  includeSummary: boolean;
  includeTranscript: boolean;
  allowUnencrypted: boolean;
}

export interface WebhookConfig extends WebhookSwitches {
  revision: string | null;
  configured: boolean;
  destination: string | null;
  signingSecret: { stored: boolean; hint: string | null };
  configurationError: string | null;
}

export interface WebhookDelivery {
  id: string;
  event: "test" | "summary.ready" | "recording.saved";
  startedAt: string;
  finishedAt: string | null;
  attempts: number;
  outcome: "pending" | "succeeded" | "failed";
  httpStatus: number | null;
  errorClass: string | null;
}

export interface WebhookSettingsView extends WebhookConfig {
  lastDelivery: WebhookDelivery | null;
}

export const WEBHOOK_CONFIG_CHANGED = "webhook-settings-changed";
export const WEBHOOK_UPDATED = "webhook-updated";

export function canSendWebhook(config: WebhookConfig | null | undefined): config is WebhookConfig & { destination: string } {
  return !!config?.enabled && !!config.configured && !!config.destination && !config.configurationError;
}

const failureMessages: Record<string, string> = {
  credentials_unavailable: "Enter the destination URL again in Settings.",
  destination: "Check that the destination is reachable and allowed by the HTTP settings.",
  meeting_unavailable: "ClawScribe could not read the saved meeting.",
  payload_too_large: "The meeting data exceeds 3 MiB.",
  network: "ClawScribe could not connect to the destination.",
  timeout: "The destination timed out.",
  redirect: "The destination redirected the request. Save the final URL in Settings.",
  settings_changed: "Webhook settings changed during delivery.",
};

export function webhookDeliveryMessage(delivery: WebhookDelivery): string {
  const status = delivery.httpStatus === null ? "" : ` (HTTP ${delivery.httpStatus})`;
  if (delivery.outcome === "succeeded") return `Delivered${status}`;
  if (delivery.outcome === "pending") return "Delivery started. Its result is unavailable if ClawScribe was closed.";
  const detail = delivery.errorClass ? failureMessages[delivery.errorClass] : undefined;
  return `Delivery failed${status}.${detail ? ` ${detail}` : ""}`;
}

export function webhookCommandError(error: unknown, fallback: string): string {
  return typeof error === "string" ? error : fallback;
}
