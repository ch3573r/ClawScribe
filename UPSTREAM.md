# Upstream Relationship

ClawScribe is based on Meetily Community Edition `0.4.0`.

Selected reliability improvements from Meetily `0.4.1` are adapted to this
fork: recoverable model downloads, HE-AAC decoded sample-rate handling,
reasoning-tag isolation, Windows CPU portability checks, and exact VAD flush
endpoints. This is not a wholesale merge of the upstream release; ClawScribe
retains its Windows GPU profile, recording lifecycle, and local engine choices.

Upstream sources:

- https://github.com/Zackriya-Solutions/meeting-minutes
- https://github.com/Zackriya-Solutions/meetily

The productized fork keeps the ClawScribe name for user-visible product surfaces while retaining Meetily references where they are needed for lawful attribution, artifact compatibility, previous-install migration, environment-variable compatibility, or upstream-source documentation.

Compatibility references intentionally retained for now include:

- legacy `meetily-recordings` folder migration behavior
- `meetily-json-v1` artifact layout names
- `openclaw.meetily-submission*.v1` handoff marker schemas
- `MEETILY_OPENCLAW_*` and `MEETILY_LLAMA_HELPER` environment variables
- legacy `Meetily/templates` custom-template fallback
- Previous Meetily installation import text and Homebrew database paths
- Upstream repository and model-host URLs

Changing these names requires a compatibility migration plan, including old-folder discovery, import/upgrade behavior, and OpenClaw ingest compatibility.
