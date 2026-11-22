# OpenAI Authentication Paths

For ChatGPT sign-in through the bundled Codex provider, see
[Codex authentication](codex-auth.md) and the
[bundled runtime guide](../codex-runtime.md). That provider uses an isolated
profile with keyring-only credentials; it does not supply ChatGPT tokens to
other providers.

Direct OpenAI Platform API-key authentication remains a separate provider.
Custom OpenAI-compatible endpoints use their own configured
credentials. Public OAuth PKCE metadata alone cannot authenticate OpenAI API
requests in ClawScribe; the compatibility mode does not exchange tokens.

Saved API keys use protected credential storage. Legacy
plaintext is migrated before use, and a failed migration prevents use of the
saved secret. See the [privacy policy](../../PRIVACY_POLICY.md#credential-storage)
for the Windows DPAPI fallback locations and data-handling limits.

Implementation: `frontend/src-tauri/src/openai/auth.rs`,
`frontend/src-tauri/src/credentials.rs`, and
`frontend/src-tauri/src/database/repositories/setting.rs`.
