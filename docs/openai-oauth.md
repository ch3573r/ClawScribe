# OpenAI Auth Modes

This fork treats OpenAI auth as a production configuration contract, not a fake
OAuth implementation.

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

## Explicit Unsupported Auth

### OAuth PKCE

Status: metadata-only, not request-ready.

The backend accepts and validates public OAuth PKCE metadata so the app can keep
a concrete future integration shape:

- `clientId`
- `authorizationEndpoint`
- `tokenEndpoint`
- `redirectUri`
- `scopes`
- optional `deviceAuthorizationEndpoint`
- optional `issuer`
- optional `audience`

The backend can prepare a real short-lived PKCE S256 browser authorization
request with:

- generated `state`
- generated `nonce`
- generated `codeVerifier`
- derived `codeChallenge`
- `codeChallengeMethod: "S256"`
- browser-launch `authorizationUrl`
- 10-minute `expiresAt`

Command:

- `api_prepare_openai_oauth_pkce_authorization`

This command does not open the browser by itself and does not store tokens.

Token exchange is intentionally unsupported in this build. The explicit
unsupported command is:

- `api_exchange_openai_oauth_pkce_code`

It returns an error instead of minting, accepting, storing, or pretending to
refresh OAuth tokens.

Before OAuth can become request-ready, the product still needs official OpenAI
OAuth app/client details for this desktop app, allowed redirect URI rules,
authorization and token endpoint requirements, scopes, and a secure local token
storage design. Until then, `canAuthenticateRequests` is false for
`oauth_pkce`.

No OAuth client secret, access token, refresh token, or fake token is stored in
source code or settings.

## Disabled Or Not Configured

When no OpenAI auth metadata and no API key are present,
`api_get_openai_auth_status` reports `disabled` with `configured: false`.

## Backend Status Fields

`api_get_openai_auth_status` returns status without returning secrets.

Important fields:

- `mode`: `disabled`, `api_key`, or `oauth_pkce`.
- `configured`: whether the selected mode has enough local configuration.
- `apiKeyPresent`: whether the OpenAI API key exists.
- `oauthPkceConfigured`: whether OAuth PKCE metadata exists.
- `oauthBrowserLaunchReady`: whether an authorization URL can be prepared.
- `oauthDeviceFlowConfigured`: whether a device endpoint was configured.
- `canAuthenticateRequests`: true only for API-key auth with a stored key.
- `requestAuthentication`: `bearer_api_key`, `missing_api_key`,
  `unsupported_oauth_pkce`, `disabled`, or `not_configured`.
- `unsupportedReason`: why OAuth cannot authenticate OpenAI requests yet.
- `nextAction`: operator-facing next step.

## Validation Notes

The Rust unit tests cover legacy API-key compatibility, disabled mode, OAuth
metadata normalization, URL validation, status capability fields, and PKCE S256
authorization request generation.
