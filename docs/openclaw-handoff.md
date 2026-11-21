# OpenClaw Handoff

ClawScribe can send saved meeting transcripts and details to the configured
OpenClaw ingest endpoint. Automatic handoff runs after a live recording saves
its library entry and recording-folder files. Open a saved meeting to review
its handoff status or submit it manually.

## Configuration

ClawScribe reads this JSON file from its Tauri app config directory. The
Windows production app identifier is `net.rismondo.openclaw.clawscribe`; use
the Settings page to see the exact resolved config path on the recorder.

```json
{
  "enabled": true,
  "allow_unencrypted": true,
  "endpoint": "http://openclaw.local:8765/meetings/completed",
  "model_endpoint": "http://openclaw.local:8765/v1/chat/completions",
  "bearer_token": "",
  "source": "ClawScribe",
  "include_audio_path": false
}
```

Use the `openclaw.local` examples as placeholders for your own OpenClaw host.
Use `http://127.0.0.1:8765/meetings/completed` only when the OpenClaw ingest
service runs on the same Windows machine.

When the summarization provider is set to `OpenClaw managed auth`, ClawScribe
uses `model_endpoint` as an OpenAI-compatible chat-completions bridge. The
example is `http://openclaw.local:8765/v1/chat/completions`; configure your own
endpoint (none is prefilled). The endpoint is implemented by the OpenClaw ingest service and routes through the host's
existing OpenClaw gateway/Codex auth. ClawScribe sends only the configured
handoff bearer token and does not store ChatGPT/Codex tokens.

The same values can be overridden with environment variables:

- `MEETILY_OPENCLAW_ENABLED`
- `MEETILY_OPENCLAW_ENDPOINT`
- `MEETILY_OPENCLAW_MODEL_ENDPOINT`
- `MEETILY_OPENCLAW_BEARER_TOKEN`
- `MEETILY_OPENCLAW_SOURCE`
- `MEETILY_OPENCLAW_INCLUDE_AUDIO_PATH`

The `MEETILY_OPENCLAW_*` names are intentionally retained as implementation
keys for reset-safe compatibility with existing recorder deployments. The
Windows package identity is now ClawScribe, but the environment variable names
remain unchanged.

Enter the bearer token in Settings. ClawScribe stores a provider-scoped OS
credential reference in `bearer_token`; a Windows fallback uses current-user
DPAPI encryption. Existing plaintext configuration is migrated before use, even
when an environment override is present. Failed or corrupt protected storage
requires reconnecting the provider; it does not silently discard the settings.
Environment overrides remain available for operator-managed launches. Keep
secrets out of copied examples and committed files.

## OpenClaw Ingestion Point Setup

The ingestion point is an operator-run OpenClaw service that accepts completed
meeting payloads from ClawScribe. It is not bundled into the ClawScribe desktop
installer; configure it on the host that should receive completed meetings.

For LAN intake, configure the ingest service environment on the OpenClaw host:

```ini
MEETING_OPENCLAW_INGEST_HOST=0.0.0.0
MEETING_OPENCLAW_INGEST_PORT=8765
MEETING_OPENCLAW_INGEST_OUTPUT_ROOT=/path/to/openclaw-ingest/out
MEETING_OPENCLAW_INGEST_REQUIRE_TOKEN=1
MEETING_OPENCLAW_INGEST_TOKEN=<long-random-token>
MEETING_OPENCLAW_MODEL_PROCESSING_ENABLED=1
MEETING_OPENCLAW_MODEL_PROCESSING_TRANSPORT=gateway
MEETING_OPENCLAW_MODEL_PROCESSING_MODEL=openai/gpt-5.4
```

Use `127.0.0.1` instead of `0.0.0.0` for loopback-only testing. If binding to
LAN, restrict source access at the host firewall or upstream firewall to the
Windows recorder subnet or host.

Install or update the ingest systemd service from the ingest repository:

```bash
cd /path/to/openclaw-ingest
sudo ./scripts/install-systemd-service.sh
sudoedit /etc/openclaw-ingest.env
sudo systemctl restart openclaw-ingest.service
curl -sS http://127.0.0.1:8765/readyz
```

From the Windows recorder, validate reachability before enabling automatic
handoff:

```powershell
Invoke-RestMethod http://openclaw.local:8765/readyz
```

Then configure the same host and bearer token in ClawScribe Settings. The
handoff endpoint should end with `/meetings/completed`; the optional model
endpoint should expose an OpenAI-compatible `/v1/chat/completions` route when
using `OpenClaw managed auth` as a summary provider.

## Behavior

ClawScribe skips meetings with empty transcript text. Each request contains a
`meeting.completed` payload and a stable idempotency key, also sent in the
`Idempotency-Key` header. Configure the ingest service to deduplicate with this
key. An HTTP success means the endpoint accepted the handoff. Settings shows
configuration and the latest delivery result independently for the current URL.

Each meeting's handoff is saved in its local library. Previously submitted
meetings are protected against repeat sends. ClawScribe reads older folder
markers and the previous Settings status file for compatibility.

Open a saved meeting and choose **Review retry** after a definite rejection or
connection failure. If delivery is uncertain after a timeout, interruption, or
ambiguous reply, choose **Review delivery**. Wait for processing to settle at
the original destination and verify whether it received that meeting. Record
**Confirmed received** to close the handoff or **Confirmed absent** to unlock a
reviewed retry to the original destination. A retry after an uncertain result
must match the saved payload hash when one is available. There is no automatic
resend on restart or after an uncertain result. The local delivery history is
outside portable backups.

The Tauri commands use saved meeting IDs to look up verified folder artifacts.

- `get_openclaw_config_status` returns configuration readiness in `ready` and
  the latest delivery state in `delivery_state` and `last_submission`.
- `get_openclaw_submission_status` takes `meetingId` and returns the saved
  result, available actions and a `review_token` for the current destination.
- `submit_meeting_folder_to_openclaw` takes `meetingId` and `reviewToken` after
  the user approves the displayed destination. It returns the meeting status.
- `reconcile_openclaw_handoff` takes `meetingId` and `received` after the user
  verifies the uncertain outcome at OpenClaw. It records the result locally.

The companion Windows tray/agent is no longer needed for the happy path once
this fork is used as the recorder. It can remain as a diagnostic/manual
processor outside the recorder's production submission path.

## Local-network HTTP opt-in

OpenClaw and custom OpenAI settings allow private-network HTTP only with
**Allow unencrypted HTTP to this local-network server** enabled. The token/key
is sent without encryption. Prefer HTTPS. Loopback HTTP needs no opt-in; public
HTTP is rejected on save/send; saved settings remain editable with a warning.
Tailscale's `100.64.0.0/10` range and `.ts.net` names count as local destinations.
Before each HTTP request, all resolved addresses must
be loopback or private, and the connection uses that checked resolution without
redirects or a proxy. Existing private HTTP configurations migrate to opted-in
on their first load; explicit choices are preserved thereafter.
Rejected saved endpoints return a `destination_problem` alongside their settings.
Correct the endpoint and save; an empty token field retains the existing token.

The OpenClaw `allow_unencrypted` JSON flag covers both handoff and model
endpoints. Custom OpenAI configuration has its own `allow_unencrypted` flag.
Confluence stores the choice with the PAT's saved origin and restores it in
Settings without needing a successful connection to the server.
