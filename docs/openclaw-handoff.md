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
  "endpoint": "http://openclaw-host.local:8765/meetings/completed",
  "bearer_token": "replace-me",
  "source": "ClawScribe",
  "include_audio_path": false
}
```

`http://openclaw-host.local:8765/meetings/completed` is the production default in
this fork. Use `http://127.0.0.1:8765/meetings/completed` only when the
OpenClaw ingest service runs on the same Windows machine.

The same values can be overridden with environment variables:

- `MEETILY_OPENCLAW_ENABLED`
- `MEETILY_OPENCLAW_ENDPOINT`
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

## OpenClaw Ingest Endpoint

The live OpenClaw-side ingest service is in:

```text
/path/to/openclaw-ingest
```

For LAN intake, the service environment should be shaped like this on the
OpenClaw host:

```ini
MEETING_OPENCLAW_INGEST_HOST=0.0.0.0
MEETING_OPENCLAW_INGEST_PORT=8765
MEETING_OPENCLAW_INGEST_OUTPUT_ROOT=/path/to/openclaw-ingest/out
MEETING_OPENCLAW_INGEST_REQUIRE_TOKEN=1
MEETING_OPENCLAW_INGEST_TOKEN=<long-random-token>
```

Use `127.0.0.1` instead of `0.0.0.0` for loopback-only testing. If binding to
LAN, restrict source access at the host firewall or upstream firewall to the
Windows recorder subnet or host.

Install or update the ingest systemd service from the ingest repo:

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
Invoke-RestMethod http://openclaw-host.local:8765/readyz
```

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
