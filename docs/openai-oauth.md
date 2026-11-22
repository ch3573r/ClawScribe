# OpenAI Auth Modes

> Update: ClawScribe now supports interactive OpenAI/ChatGPT login through
> Codex. See `docs/auth/codex-auth.md`. The OAuth/PKCE notes below still apply
> to direct OpenAI API credentials: ClawScribe must not use private/raw OpenAI
> OAuth endpoints or treat Codex tokens as generic API bearer tokens.

ClawScribe supports direct OpenAI API-key auth, custom OpenAI-compatible
endpoints. Public OpenAI OAuth PKCE
metadata is kept as compatibility metadata only; it is not treated as a usable
OpenAI API credential.

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

### Standalone OpenAI-Compatible Managed Endpoint

Status: supported through the `custom-openai` provider.

This is the distributable path for an OAuth-backed or operator-managed backend:
the backend owns OAuth/token refresh/secrets, exposes an OpenAI-compatible
`/chat/completions` endpoint, and ClawScribe sends bearer-authenticated requests
to that endpoint. The endpoint can be local, LAN, cloud, or vendor-hosted.

Configuration shape:

```json
{
  "provider": "custom-openai",
  "customOpenAIEndpoint": "https://model-gateway.example.com/v1",
  "customOpenAIModel": "gpt-5.4",
  "customOpenAIApiKey": "gateway-bearer-token"
}
```

Related commands:

- `api_save_custom_openai_config`
- `api_get_custom_openai_config`
- `api_test_custom_openai_connection`

## Public OpenAI OAuth PKCE Metadata

Status: compatibility metadata only, not request-ready by itself.

Public OpenAI OAuth PKCE metadata is intentionally distinct from both standalone
OpenAI API-key auth and managed OpenAI-compatible endpoints. The backend may
still validate legacy PKCE metadata for compatibility, but it cannot launch a
browser flow or exchange codes. That metadata alone does not authenticate
ClawScribe requests. For OAuth-backed processing, use a standalone
OpenAI-compatible managed endpoint.

## Disabled Or Not Configured

When no OpenAI auth metadata and no API key are present,
`api_get_openai_auth_status` reports `disabled` with `configured: false`.

## Backend Status Fields

`api_get_openai_auth_status` returns status without returning secrets.

Important fields:

- `mode`: `disabled`, `api_key`, or `oauth_pkce`.
- `configured`: whether the selected mode has enough local configuration.
- `apiKeyPresent`: whether the OpenAI API key exists.
- `oauthPkceConfigured`: whether legacy public OAuth PKCE metadata exists.
- `oauthBrowserLaunchReady`: false; legacy OAuth browser commands are not supported.
- `oauthDeviceFlowConfigured`: whether a legacy device endpoint was configured.
- `canAuthenticateRequests`: true for API-key auth with a stored key. Custom OpenAI-compatible
  endpoint readiness is reported by the custom provider config/test commands.
- `requestAuthentication`: `bearer_api_key`, `missing_api_key`,
  `unsupported_oauth_pkce`, `disabled`, or `not_configured`.
- `unsupportedReason`: why legacy public OAuth PKCE cannot authenticate
  requests.
- `nextAction`: operator-facing next step.

## Validation Notes

The Rust unit tests cover legacy API-key compatibility, disabled mode, legacy
public OAuth metadata handling, URL validation, and PKCE S256 authorization
request compatibility.

## Local-network HTTP opt-in

Custom OpenAI settings allow private-network HTTP only with
**Allow unencrypted HTTP to this local-network server** enabled. The token/key
is sent without encryption. Prefer HTTPS. Loopback HTTP needs no opt-in; public
HTTP is rejected on save/send; saved settings remain editable with a warning.
Tailscale's `100.64.0.0/10` range and `.ts.net` names count as local destinations.
Before each HTTP request, all resolved addresses must
be loopback or private, and the connection uses that checked resolution without
redirects or a proxy. Existing private HTTP configurations migrate to opted-in
on their first load; explicit choices are preserved thereafter.

Custom OpenAI configuration stores the choice in `allow_unencrypted`.
Confluence stores the choice with the PAT's saved origin.
