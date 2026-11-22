# Outbound webhook

ClawScribe can POST saved meeting data to one automation endpoint. Open
**Settings → Webhook**, enter the destination URL and select **Enable webhook**.
Review when to send and what to include, then select **Save**. The webhook is off
until you enable and save it.

| Action | Event | Default |
| --- | --- | --- |
| Complete a summary, including automatic generation or regeneration | `summary.ready` | On when enabled |
| Save a recording or finish importing an audio file | `recording.saved` | Off |
| Select **Send to webhook** in a meeting's export menu | `summary.ready` | On demand |
| Select **Send test** in Settings | `test` | On demand |

The two automatic triggers are independent. With both on, the receiver can get
a recording event followed by a summary event for the same meeting. A recording
event can include the available text of a partially transcribed recording.
If the recording audio fails to save, ClawScribe skips the recording event.
Editing notes, restoring historical data and repairing recording files do not
send a webhook. A test uses a fixed sample meeting without real meeting data.

## Destination and privacy

Use an HTTPS URL such as `https://hooks.example.com/webhook/meeting-ready`.
Local-network or loopback HTTP requires **Allow unencrypted HTTP to this
local-network server**. Public HTTP, credentials in URL userinfo and redirects
are rejected. HTTP exposes the request path, query and meeting data to the
network. The signing secret authenticates a request and HTTPS protects its
contents in transit.

Treat the full URL as a password, especially when it contains a token in its
path or query. ClawScribe stores it with the optional signing secret in the
system credential store. On Windows, the fallback uses current-user DPAPI
encryption. Settings show the scheme, host and port with the rest masked.
When you replace the URL, enter a signing secret for the new destination if it
requires one. **Remove** clears the saved destination and signing secret.

Every meeting delivery includes meeting details. Summary and action items are
included by default. The saved transcript, including edited text and assigned
speaker names, is included only if you enable it. The payload has no audio,
local file metadata, transcript passage IDs, attendee contact addresses or
internal citation tags. The receiver can store and share the data according to
its own settings. **Settings → Privacy** shows the webhook destination and
what ClawScribe sends.

## Request format

ClawScribe sends an HTTP `POST` with these headers.

| Header | Value |
| --- | --- |
| `Content-Type` | `application/json` |
| `User-Agent` | `ClawScribe/<app_version>` |
| `X-ClawScribe-Event` | `summary.ready`, `recording.saved` or `test` |
| `X-ClawScribe-Delivery` | The delivery UUID in the body |
| `X-ClawScribe-Timestamp` | Unix time in seconds. Present with a signing secret. |
| `X-ClawScribe-Signature` | `sha256=<lowercase hex digest>`. Present with a signing secret. |

```json
{
  "schema": "clawscribe.webhook.v1",
  "event": "summary.ready",
  "delivery_id": "00000000-0000-4000-8000-000000000001",
  "sent_at": "2026-01-01T12:10:00.000Z",
  "app_version": "0.66.1",
  "meeting": {
    "id": "example-meeting",
    "title": "Planning",
    "started_at": "2026-01-01T12:00:00Z",
    "duration_seconds": 600,
    "project_tags": ["Planning"],
    "attendees": ["Alex"]
  },
  "summary_markdown": "## Overview\nThe team reviewed the plan.",
  "action_items": [{"text": "Share the plan", "owner": "Alex", "due": "2026-01-02"}],
  "transcript": [{"start_seconds": 4.5, "end_seconds": 7, "speaker": "Alex", "text": "Let's share the plan."}]
}
```

`sent_at` is RFC 3339 UTC. When an older import stores a date without a time,
`meeting.started_at` is an ISO calendar date. Unknown `duration_seconds` or
transcript times are `null`. Project tags and reviewed attendee names are
omitted when absent, as are the optional `owner`, `due` and `speaker` fields.
Optional content fields are omitted when their switch is off or the data is
absent. The request body cap is 3 MiB. If the transcript would put the request
over the cap, the entire transcript is omitted and `transcript_truncated` is
set to `true`. If the remaining body exceeds the cap, the delivery fails
locally and appears in Settings.

## Verify a signed request

The signing key is the UTF-8 encoding of the configured secret. ClawScribe
computes HMAC-SHA256 over the ASCII timestamp header, a literal period
(`0x2e`), and the exact JSON body bytes in that order. Verify the raw bytes
before parsing or reformatting the JSON. This Node.js example expects the
receiver's lowercase header keys and a `Buffer` for the request body.

```js
import { createHmac, timingSafeEqual } from "node:crypto";

export function verifyWebhook(rawBody, headers, secret) {
  const timestamp = headers["x-clawscribe-timestamp"];
  const signature = headers["x-clawscribe-signature"];
  if (typeof timestamp !== "string" || typeof signature !== "string" ||
      !/^\d{1,12}$/.test(timestamp) || !/^sha256=[0-9a-f]{64}$/.test(signature)) {
    return false;
  }
  if (Math.abs(Date.now() / 1000 - Number(timestamp)) > 300) {
    return false;
  }
  const expected = createHmac("sha256", secret)
    .update(timestamp, "ascii")
    .update(".")
    .update(rawBody)
    .digest();
  return timingSafeEqual(Buffer.from(signature.slice(7), "hex"), expected);
}
```

Supply the key from your receiver's protected configuration. After verification,
use the authenticated body's `delivery_id` to deduplicate retries and replays
before taking action. Use the body's `event` to route requests and ignore
`test` when a workflow has real side effects.

## Retries and delivery results

ClawScribe treats HTTP 2xx as acceptance. Network errors, timeouts, HTTP 408,
429 and 5xx can retry twice, after 250 ms and 500 ms. Delivery uses a 30-second
total budget with up to nine seconds per attempt. All attempts reuse the same
delivery ID, body and signature. Redirects and other HTTP failures end the
delivery immediately. ClawScribe reads the status and discards the response
body. Settings changes stop pending attempts. Interrupted deliveries are
not replayed after a restart.

**Last delivery** shows the event, time, result, attempt count and HTTP status
when available. The local history records delivery metadata and a redacted
failure category. It excludes the destination URL, signing secret and payload.

## Connect a receiver

- In n8n, add a **Webhook** trigger with the POST method. Use its test URL
  while listening for a test event. Publish the workflow and switch
  ClawScribe to the production URL, for example
  `https://hooks.example.com/webhook/meeting-ready`. [n8n Webhook node](https://docs.n8n.io/integrations/builtin/core-nodes/n8n-nodes-base.webhook/)
- In Zapier, add a **Webhooks by Zapier → Catch Hook** trigger for parsed JSON,
  or use **Catch Raw Hook** if you need request headers and raw body bytes.
  Copy the generated URL, for example
  `https://hooks.example.com/hooks/catch/example/meeting-ready/`, to ClawScribe.
  Use **Send test** to give the trigger a sample. Zapier's raw hook can have a
  smaller body limit than ClawScribe. [Zapier webhook triggers](https://help.zapier.com/hc/en-us/articles/8496288690317-Trigger-Zaps-from-webhooks)
