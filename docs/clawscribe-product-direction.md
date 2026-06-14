# ClawScribe Product Direction

ClawScribe is the product name for this OpenClaw-focused Meetily fork: a
bot-free Teams meeting recorder that captures the user's local microphone and
system audio without joining the meeting as a visible participant. The product
keeps the upstream Meetily local-first recording and transcription base, then
adds a reliable OpenClaw handoff for post-meeting processing.

## Target Workflow

1. ClawScribe records a Teams meeting from the user's Windows session using
   local audio capture.
2. The recorder writes Meetily-format artifacts such as `metadata.json`,
   `transcripts.json`, and optional local audio paths.
3. The OpenClaw handoff posts a `meeting.completed` payload to a local or
   network OpenClaw ingest endpoint.
4. OpenClaw handles summarization, action extraction, storage, and user
   notification.

## Auth Direction

ClawScribe should support OpenAI authentication in the ways operators actually
need:

- A direct OpenAI API key for simple single-user deployments.
- An OpenAI-compatible endpoint for gateways such as LiteLLM or OpenClaw-managed
  model routing.
- Local-only transcription and deferred OpenClaw processing when no cloud model
  credential should live on the recorder.

## Teams Detection

The near-term product can rely on manual start/stop and OpenClaw handoff. Future
meeting detection should be added from the logged-in Windows session, not as a
tenant-level Microsoft Graph dependency. Candidate signals include Teams window
state, active audio sessions, calendar context, and confidence-scored heuristics
before automatically starting or stopping a recording.

## Implementation Guardrails

- Keep ClawScribe as the fork-facing product name while upstream Meetily remains
  the implementation base for artifact formats and build structure.
- Use `net.rismondo.openclaw.clawscribe` for the Windows package identity.
  Keep `MEETILY_OPENCLAW_*` environment variables for deployment compatibility.
- Keep the OpenClaw handoff reset-safe: artifact markers should live inside the
  recording folder, and repeated submissions should remain idempotent.
