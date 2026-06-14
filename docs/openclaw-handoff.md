# OpenClaw Handoff

This fork can submit completed Meetily recording folders directly to the
OpenClaw meeting ingest endpoint. The handoff runs after Meetily finishes
stopping a recording and has written final `transcripts.json` and
`metadata.json`.

## Configuration

Meetily reads this JSON file from its Tauri app config directory:

```json
{
  "enabled": true,
  "endpoint": "http://127.0.0.1:8765/meetings/completed",
  "bearer_token": "replace-me",
  "source": "meetily-openclaw",
  "include_audio_path": false
}
```

The same values can be overridden with environment variables:

- `MEETILY_OPENCLAW_ENABLED`
- `MEETILY_OPENCLAW_ENDPOINT`
- `MEETILY_OPENCLAW_BEARER_TOKEN`
- `MEETILY_OPENCLAW_SOURCE`

## Behavior

When a recording stops, Meetily builds a `meeting.completed` payload from the
recording folder and posts it to the configured endpoint with bearer auth.

On success, Meetily writes `.openclaw-submitted.json` into the recording folder.
On failure, it writes `.openclaw-failed.json`.

The companion Windows tray/agent is no longer needed for the happy path once
this fork is used as the recorder. It can remain as a diagnostic/manual
processor while the fork-native handoff is being hardened.
