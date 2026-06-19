# ClawScribe

![ClawScribe README hero](docs/brand/clawscribe-readme-hero.png)

ClawScribe is a local-first desktop recorder for meetings, calls, interviews,
and long-form audio. It captures microphone and system audio from the user
session, transcribes speech locally, and turns transcripts into useful notes,
summaries, follow-ups, and exports.

Current version: `0.5.1-alpha.2`

ClawScribe is based on Meetily Community Edition `0.4.0`. Upstream attribution
and license details are in [UPSTREAM.md](UPSTREAM.md), [NOTICE.md](NOTICE.md),
and [LICENSE.md](LICENSE.md).

## What ClawScribe Does

- Records microphone and system audio without joining meetings as a bot.
- Transcribes locally with on-device engines such as Whisper, Parakeet, and
  Nemotron paths.
- Generates summaries through built-in/local AI, OpenAI API keys,
  OpenAI-compatible endpoints, or optional OpenClaw-managed processing.
- Keeps meeting artifacts in a local Meetily-compatible folder structure.
- Exports to Microsoft OneNote and Planner through Microsoft Graph when the
  user signs in and chooses destinations.
- Supports Windows-focused release builds with CPU, Vulkan, CUDA, OpenBLAS, and
  DirectML feature paths depending on the model engine.

## Product Status

ClawScribe is currently an alpha productization fork. Treat builds as validation
artifacts until a signed public release pipeline and final credential-storage
migration are complete.

Current boundaries:

- The Tauri desktop app is the supported runtime.
- The legacy Python/FastAPI backend under `backend/` is retained only as
  historical reference and migration context.
- Some compatibility names, environment variables, and recording folders still
  use Meetily naming to preserve existing local data.
- Optional OpenClaw/OpenAI-compatible endpoints must be configured by the user
  or operator. No private endpoint is baked into the app.

## Repository Layout

```text
frontend/                  Next.js UI and Tauri desktop app
frontend/src-tauri/src/    Rust app core, audio, transcription, exports, summary
frontend/src/              React components, hooks, services, and app routes
backend/                   Legacy backend archive, not the supported runtime
docs/                      Product, build, verification, and integration notes
llama-helper/              Local summary sidecar helper
scripts/                   Repository utility scripts
```

## Development

Install frontend dependencies and run the Tauri app:

```bash
cd frontend
pnpm install
pnpm run tauri:dev
```

Run the web UI only:

```bash
cd frontend
pnpm run dev
```

Build the desktop app:

```bash
cd frontend
pnpm run tauri:build
```

Windows release validation and packaging:

```powershell
cd frontend
.\scripts\build-windows-release.ps1 -CheckOnly
.\scripts\build-windows-release.ps1
```

GPU-specific developer scripts are available in `frontend/`:

```text
dev-gpu.*
build-gpu.*
```

Use those only when validating acceleration paths.

## Privacy And Credentials

ClawScribe is designed around local capture and local-first processing, but it
can optionally call user-configured AI and export providers.

Security rules for contributors:

- Do not commit `.env` files, tokens, API keys, bearer strings, certificates,
  local auth stores, logs, databases, generated installers, or local build
  tool installers.
- Commit placeholders only in `.env.example` files.
- Use example hosts such as `openclaw.local` or `example.com` in docs instead
  of private LAN addresses.
- Keep app logs, tests, analytics, and error messages redacted.

## OpenClaw And Provider Integrations

Optional OpenClaw handoff is documented in
[docs/openclaw-handoff.md](docs/openclaw-handoff.md).

OpenAI and OpenAI-compatible auth behavior is documented in
[docs/openai-oauth.md](docs/openai-oauth.md) and
[docs/auth/openai-login.md](docs/auth/openai-login.md).

Microsoft export setup and verification notes are under `docs/integrations/`
and `docs/verification/`.

## Support

ClawScribe remains MIT licensed and free to use, modify, and redistribute under
the terms in [LICENSE.md](LICENSE.md). If it saves you time or you want to
support local-first meeting AI work, you can support development here:

[Buy me a coffee](https://buymeacoffee.com/ch3573r)

## Build Documentation

- [docs/BUILDING.md](docs/BUILDING.md)
- [docs/windows-release.md](docs/windows-release.md)
- [docs/GPU_ACCELERATION.md](docs/GPU_ACCELERATION.md)
- [frontend/README.md](frontend/README.md)

## License

ClawScribe remains under the MIT License. See [LICENSE.md](LICENSE.md).

Upstream Meetily code is copyright Zackriya Solutions and contributors.
ClawScribe changes are copyright OpenClaw contributors unless otherwise noted.
