# OpenAI Auth Modes

This fork treats OpenAI auth as a production configuration contract. ClawScribe
supports direct OpenAI API-key auth, and a separate OpenClaw/Codex managed route
for ChatGPT/Codex-authenticated processing. The managed route is not public
OpenAI OAuth.

## Supported Request Auth

### API Key

Status: supported and request-ready.

The `openai` provider uses the existing `openaiApiKey` value in the local
settings database. Summary generation and model-list calls authenticate OpenAI
API requests with HTTP bearer auth using that API key.

The OpenAI API reference documents bearer credentials for API requests and warns
that API keys are secrets that should not be exposed in client-side code:

- https://developers.openai.com/api/reference/overview#authentication

This desktop app stores the key locally through the existing settings path. The
OpenAI auth status endpoint reports whether the key exists, but never returns it.

Related commands:

- `api_save_model_config`
- `api_get_model_config`
- `api_get_api_key`
- `get_openai_models`
- `api_get_openai_auth_status`
- `api_save_openai_auth_config` with `{ "mode": "api_key" }`

### OpenClaw/Codex Managed Auth

Status: supported and request-ready when an endpoint is configured.

This mode represents a production handoff to an OpenClaw endpoint or local
backend that owns ChatGPT/Codex-authenticated processing. ClawScribe stores only
endpoint metadata and does not mint, accept, persist, refresh, or expose ChatGPT
or Codex tokens.

In the current OpenClaw deployment, this endpoint is the ingest service's
OpenAI-compatible model bridge:

```text
http://openclaw-host.local:8765/v1/chat/completions
```

That bridge runs `openclaw infer model run --gateway --model openai/gpt-5.4`
on the trusted OpenClaw host, using the existing OpenClaw/ChatGPT/Codex auth
state there.

Configuration shape:

```json
{
  "mode": "openclaw_codex_managed",
  "openclawCodexManaged": {
    "endpoint": "http://openclaw-host.local:8765/v1/chat/completions",
    "statusEndpoint": "http://openclaw-host.local:8765/readyz",
    "label": "OpenClaw managed auth bridge"
  }
}
```

Fields:

- `endpoint`: required OpenClaw/local backend endpoint used for managed
  ChatGPT/Codex-authenticated processing.
- `statusEndpoint`: optional endpoint for backend sign-in/readiness checks.
- `label`: optional operator-facing name for the configured backend.

URLs must use `https`, except localhost and `127.0.0.1` endpoints may use
`http` for local desktop backends.

Related command:

- `api_save_openai_auth_config` with `{ "mode": "openclaw_codex_managed" }`

## Public OpenAI OAuth PKCE

Status: compatibility metadata only, not request-ready.

Public OpenAI OAuth PKCE is intentionally distinct from the OpenClaw/Codex
managed route. The backend may still validate legacy PKCE metadata and prepare a
short-lived browser authorization URL for compatibility, but that path does not
authenticate ClawScribe requests.

Compatibility commands:

- `api_prepare_openai_oauth_pkce_authorization`
- `api_exchange_openai_oauth_pkce_code`

`api_exchange_openai_oauth_pkce_code` returns an error. No OAuth client secret,
access token, refresh token, ChatGPT token, Codex token, or fake token is stored
in source code or settings.

## Disabled Or Not Configured

When no OpenAI auth metadata and no API key are present,
`api_get_openai_auth_status` reports `disabled` with `configured: false`.

## Backend Status Fields

`api_get_openai_auth_status` returns status without returning secrets.

Important fields:

- `mode`: `disabled`, `api_key`, `openclaw_codex_managed`, or `oauth_pkce`.
- `configured`: whether the selected mode has enough local configuration.
- `apiKeyPresent`: whether the OpenAI API key exists.
- `openclawCodexManagedConfigured`: whether managed endpoint metadata exists.
- `openclawCodexEndpointPresent`: whether the managed request endpoint exists.
- `oauthPkceConfigured`: whether legacy public OAuth PKCE metadata exists.
- `oauthBrowserLaunchReady`: whether a legacy authorization URL can be prepared.
- `oauthDeviceFlowConfigured`: whether a legacy device endpoint was configured.
- `canAuthenticateRequests`: true for API-key auth with a stored key, or for
  `openclaw_codex_managed` with an endpoint configured.
- `requestAuthentication`: `bearer_api_key`, `missing_api_key`,
  `openclaw_codex_managed`, `missing_openclaw_codex_endpoint`,
  `unsupported_oauth_pkce`, `disabled`, or `not_configured`.
- `unsupportedReason`: why legacy public OAuth PKCE cannot authenticate
  requests.
- `nextAction`: operator-facing next step.

## Validation Notes

The Rust unit tests cover legacy API-key compatibility, disabled mode,
OpenClaw/Codex managed endpoint normalization and status capability fields,
legacy public OAuth metadata handling, URL validation, and PKCE S256
authorization request compatibility.
