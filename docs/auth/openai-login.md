# OpenAI Login Plan

> Superseded for ClawScribe's interactive OpenAI sign-in path by
> `docs/auth/codex-auth.md`.
>
> The supported login implementation is now OpenAI login via Codex. ClawScribe
> still must not implement private/raw OpenAI OAuth endpoints, scrape ChatGPT
> cookies, or extract Codex tokens for generic API bearer use. Direct OpenAI
> Platform API-key mode remains available as a separate provider.

Date: 2026-06-15
Product: ClawScribe
Branch: `feat/clawscribe-productization-auth-theme-exports`

## Decision

ClawScribe should not implement a standalone "Sign in with OpenAI" or "Sign in
with ChatGPT" flow in the desktop app for OpenAI API access in this pass.

Supported production paths are:

1. Direct OpenAI Platform API key auth.
2. A standalone OpenAI-compatible managed endpoint that owns any OAuth, token
   refresh, key management, and provider policy.
3. Optional OpenClaw managed auth, where ClawScribe sends OpenAI-compatible
   requests to the configured OpenClaw bridge and the trusted OpenClaw host owns
   Codex/ChatGPT auth.

Blocked paths are:

1. Inventing private OpenAI OAuth endpoints.
2. Scraping or automating the ChatGPT web UI.
3. Copying, storing, or refreshing ChatGPT/Codex tokens inside ClawScribe.
4. Treating generic OAuth PKCE metadata as enough to authenticate OpenAI API
   requests.

## Official OpenAI Evidence

The OpenAI API reference says API requests accept bearer credentials from API
keys or short-lived access tokens created by workload identity federation, and
shows the request header as:

```text
Authorization: Bearer OPENAI_API_KEY_OR_ACCESS_TOKEN
```

It also warns that API keys are secrets and should not be exposed in client-side
code such as browsers or apps. Source:
https://developers.openai.com/api/reference/overview#authentication

Workload identity federation is service-account/project backed. OpenAI documents
those access tokens as authorizing like service-account API credentials, not user
OAuth tokens. Source:
https://developers.openai.com/api/reference/workload-identity-federation

Codex supports ChatGPT sign-in and API-key sign-in, but that is a Codex product
auth path. The Codex docs say login details are cached locally in `auth.json` or
an OS credential store, and Codex refreshes ChatGPT sessions during use. They
also state that Codex account-auth automation is advanced, trusted-runner-only,
and does not apply to generic OAuth clients outside Codex. Sources:
https://developers.openai.com/codex/auth and
https://developers.openai.com/codex/auth/ci-cd-auth

The Apps SDK / ChatGPT connector OAuth flow is the inverse direction:
ChatGPT acts as an OAuth client to call an app's MCP server, and the app verifies
tokens on incoming tool requests. That does not provide a desktop app with a
general OpenAI API OAuth client. Source:
https://developers.openai.com/apps-sdk/build/auth

## Current ClawScribe Behavior

Backend auth mode code is in `frontend/src-tauri/src/openai/auth.rs`.

Observed behavior:

- `api_key` is request-ready only when a stored OpenAI API key is present.
- `openclaw_codex_managed` is request-ready when an OpenClaw model endpoint is
  configured.
- `oauth_pkce` can validate metadata and prepare a PKCE S256 authorization URL,
  but `api_exchange_openai_oauth_pkce_code` always returns an unsupported error.
- The backend explicitly says public OpenAI OAuth PKCE metadata alone cannot
  authenticate OpenAI API requests.
- No OAuth client secret, access token, refresh token, ChatGPT token, Codex
  token, or fake token is accepted or stored by this module.

Settings storage is in
`frontend/src-tauri/src/database/repositories/setting.rs`.

Observed behavior:

- Direct OpenAI API keys are stored in the `settings.openaiApiKey` column.
- `openAIAuthConfig` stores auth-mode metadata only.
- Custom OpenAI-compatible endpoint config is stored as JSON in
  `settings.customOpenAIConfig`, including that endpoint's bearer token when
  configured.
- The current repository path is local settings storage, not proven OS
  credential storage.

OpenClaw handoff is in `frontend/src-tauri/src/openclaw.rs`.

Observed behavior:

- ClawScribe keeps backward-compatible OpenClaw handoff config in
  `openclaw.json`.
- An example model bridge is
  `http://openclaw.local:8765/v1/chat/completions`.
- Saving an empty bearer token preserves an existing token.
- The status command returns whether a bearer token exists, not the token value.

Current UI evidence in `frontend/src/components/ModelSettingsModal.tsx`:

- The OpenAI API-key provider tells users to paste a Platform key and points
  interactive ChatGPT/OpenAI login users to the Codex provider.
- The custom OpenAI provider says an OAuth-backed deployment must own OAuth and
  token refresh behind the endpoint.
- The OpenClaw provider says OAuth stays on the OpenClaw side and ClawScribe
  stores endpoint configuration plus the handoff bearer token.

## OpenClaw And Hermes Findings

Reusable OpenClaw pattern found:

- `openclaw-ingest` exposes an OpenAI-compatible bridge at
  `POST /v1/chat/completions` protected by the ingest bearer token.
- That bridge translates the request into `openclaw infer model run --gateway`
  using the trusted OpenClaw host's existing Codex/ChatGPT auth state.
- The ingest README states it does not implement a separate OpenAI OAuth client
  and that ChatGPT/Codex tokens remain inside OpenClaw's auth store.
- Ingest tests assert `OPENAI_API_KEY` is not forwarded into the OpenClaw
  subprocess environment and that prompt command logging is redacted.

Reusable Hermes implementation found: none in the local project checkout.

Local evidence under `projects/` contains ClawScribe, the Windows meeting
capture agent, openclaw-ingest, and ops-design. No Hermes Agent
source checkout was present there. Workspace memory mentions Hermes provider and
auth setup discussions, but those are not an implementation pattern ClawScribe
can safely reuse without re-checking a live Hermes repo/config.

## API-Key Fallback

API-key auth remains the required direct OpenAI fallback.

Requirements:

- Use the existing OpenAI Platform API key path for direct OpenAI API calls.
- Never return key material from auth status commands.
- Never log full keys, bearer tokens, OAuth codes, refresh tokens, or access
  tokens.
- Treat model-list failures as non-fatal and keep fallback model choices
  available.
- In product copy, call this "OpenAI API key" or "OpenAI Platform key", not
  "OpenAI login".

## Secure Storage Requirements

Before claiming production-grade credential storage for a distributable desktop
build, move secrets out of plain app settings storage.

Minimum requirements:

- Store OpenAI API keys, custom endpoint bearer tokens, and OpenClaw bearer
  tokens in the OS credential store:
  - Windows: Credential Manager / DPAPI-protected credential storage.
  - macOS: Keychain.
  - Linux: Secret Service / libsecret when available, with a documented local
    file fallback only for development.
- Keep non-secret metadata in SQLite/settings: selected mode, provider, model,
  endpoint URL, token-present booleans, and labels.
- Preserve backward compatibility by reading existing settings/openclaw config,
  migrating secrets once, then writing only metadata back when possible.
- Redact secrets in all command arguments, logs, analytics, error messages,
  crash reports, docs, screenshots, and test fixtures.
- Do not store ChatGPT/Codex auth artifacts in ClawScribe. If Codex auth is
  needed, keep it behind an OpenClaw-managed endpoint or another trusted backend.

## Implementation Plan

This first pass should remain documentation-only. No shared settings UI changes
are required yet.

Next backend-safe implementation work:

1. Add a credential-store abstraction for provider secrets, keeping the current
   settings repository as a migration/read fallback.
2. Add redaction tests around OpenClaw config save/load, custom OpenAI config
   save/load, and auth status serialization.
3. Add a migration path that preserves existing `openaiApiKey`,
   `customOpenAIConfig.apiKey`, and `openclaw.json.bearer_token` behavior.
4. Keep `oauth_pkce` as unsupported compatibility metadata unless OpenAI ships
   an official third-party desktop/API OAuth registration path for this use
   case.
5. Coordinate with UI/theme and Microsoft workers before changing
   `ModelSettingsModal.tsx` or shared settings layout.

Potential small UI integration later:

- Keep the OpenAI provider API-key-first.
- Keep the OpenClaw provider as "OpenClaw Gateway" / "OpenClaw managed auth".
- Keep custom OpenAI-compatible endpoints as the extension point for
  OAuth-backed managed deployments.
- Avoid any "Sign in with OpenAI" button until it is backed by official OpenAI
  product support for this desktop/API use case.

## Manual Verification

Run from the ClawScribe repo:

```bash
cd frontend/src-tauri
cargo test openai::auth --lib
```

Expected result: all OpenAI auth unit tests pass, including:

- legacy API-key status reports request-ready API-key auth;
- disabled mode overrides a legacy key in status only;
- OpenClaw managed auth is request-ready without storing ChatGPT/Codex tokens;
- public OAuth PKCE metadata is not request-ready;
- PKCE authorization request uses S256 and reports token exchange unsupported.

Manual app verification on a desktop build:

1. Open settings and select `OpenAI`.
2. Confirm the UI asks for a Platform API key and does not offer ChatGPT web
   login.
3. Save a test API key, reload settings, and confirm status shows key presence
   without displaying the key unless the user explicitly reveals the input.
4. Generate a summary using the OpenAI provider.
5. Check app logs for provider/model/status only, not raw key material.

OpenClaw bridge verification from the OpenClaw host:

```bash
cd /path/to/openclaw-ingest
sudo scripts/smoke-openclaw-bridge.py
```

Expected result:

- `/healthz` succeeds without auth;
- unauthenticated `/v1/chat/completions` returns `401`;
- authenticated `/v1/chat/completions` succeeds;
- the output prints only response status, model/provider metadata, and redacted
  command details.

Regression guard:

- Do not run tests or smoke scripts with real secrets echoed on the command
  line.
- Do not paste or commit generated `auth.json`, API keys, bearer tokens, OAuth
  codes, or env files.
