# OpenClaw Handoff

ClawScribe can submit completed Meetily-format recording folders directly to
the OpenClaw meeting ingest endpoint. The handoff runs after recording stops
and final `transcripts.json` and `metadata.json` artifacts have been written.

## Configuration

ClawScribe reads this JSON file from its Tauri app config directory. The
Windows production app identifier is `net.rismondo.openclaw.clawscribe`; use
the Settings page to see the exact resolved config path on the recorder.

```json
{
  "enabled": true,
  "endpoint": "http://openclaw.local:8765/meetings/completed",
  "model_endpoint": "http://openclaw.local:8765/v1/chat/completions",
  "bearer_token": "replace-me",
  "source": "ClawScribe",
  "include_audio_path": false
}
```

Use the `openclaw.local` examples as placeholders for your own OpenClaw host.
Use `http://127.0.0.1:8765/meetings/completed` only when the OpenClaw ingest
service runs on the same Windows machine.

When the summarization provider is set to `OpenClaw managed auth`, ClawScribe
uses `model_endpoint` as an OpenAI-compatible chat-completions bridge. The
default is `http://openclaw.local:8765/v1/chat/completions`; that endpoint is
implemented by the OpenClaw ingest service and routes through the host's
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

Keep the bearer token out of committed files. On Windows, prefer setting
`MEETILY_OPENCLAW_BEARER_TOKEN` as a user environment variable and leaving the
JSON `bearer_token` blank or placeholder-only in any copied example.

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

When a recording stops, ClawScribe builds a `meeting.completed` payload from the
recording folder and posts it to the configured endpoint with bearer auth.

Before posting, ClawScribe writes `.openclaw-pending.json` into the recording
folder. A fresh pending marker suppresses duplicate in-flight submissions; a
pending marker older than 15 minutes is treated as stale and replaced.

On success, ClawScribe writes `.openclaw-submitted.json` into the recording
folder and removes stale pending or failure markers. On failure, it writes
`.openclaw-failed.json`. Already-submitted folders are skipped locally so
manual retries do not accidentally double-post a completed recording.

The Tauri app config directory also contains `openclaw-last-submission.json`,
which mirrors the most recent automatic or manual handoff outcome for the
Settings status panel.

Frontend status commands:

- `get_openclaw_config_status` returns enabled/configured state, config path,
  last-status path, token-present status, audio-path setting, ready/not-ready
  state, status message, and the last submission state if present.
- `get_openclaw_submission_status` accepts a recording folder path and returns
  the folder's pending, submitted, or failed marker state.
- `submit_meeting_folder_to_openclaw` manually submits a folder and updates the
  same markers and last-status file.

The companion Windows tray/agent is no longer needed for the happy path once
this fork is used as the recorder. It can remain as a diagnostic/manual
processor outside the recorder's production submission path.
