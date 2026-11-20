/**
 * Stored secrets in the interface.
 *
 * The core never returns a stored key, token or password. It reports, per
 * secret, only whether one is stored and a short hint (the last four characters
 * of a long enough secret). The interface holds a secret only while the user is
 * typing a new one, and sends it only when it was typed.
 */

export interface SecretStatus {
  stored: boolean;
  hint: string | null;
}

export const NO_SECRET: SecretStatus = { stored: false, hint: null };

/** Read a status from a backend value, tolerating anything unexpected. */
export function readSecretStatus(value: unknown): SecretStatus {
  if (!value || typeof value !== 'object') return NO_SECRET;
  const { stored, hint } = value as { stored?: unknown; hint?: unknown };
  if (stored !== true) return NO_SECRET;
  return { stored: true, hint: typeof hint === 'string' && hint.length > 0 ? hint : null };
}

/** The short text shown for a stored secret. */
export function describeSecret(status: SecretStatus | null | undefined): string {
  if (!status?.stored) return 'Not set';
  return status.hint ? `Saved, ends in ${status.hint}` : 'Saved';
}

/**
 * The key to send with a save: the text the user typed, trimmed, or null when
 * the field was not touched. Null means "keep the stored key": it is never a
 * request to blank it, and a hint or mask is never sent as a key.
 */
export function newKeyPayload(typed: string | null | undefined): string | null {
  const key = typed?.trim();
  return key ? key : null;
}

/** A new key shorter than this is refused (the core checks the same rule). */
export const MIN_KEY_CHARS = 8;
export const KEY_TOO_SHORT = 'That is too short to be a key. Enter the full key.';

/**
 * Why a typed key cannot be saved, or null. An empty field is not a problem
 * (it keeps the stored key); text that is too short is, so that typing the
 * four-character hint back can never replace a real key.
 */
export function keyProblem(typed: string | null | undefined): string | null {
  const key = typed?.trim();
  if (!key) return null;
  return Array.from(key).length < MIN_KEY_CHARS ? KEY_TOO_SHORT : null;
}

export const KEY_REQUIRED_FOR_NEW_ADDRESS = 'The address changed. Enter the key again so it is only sent to the new address.';

function addressParts(value: string): { scheme: string; host: string; port: string; explicitPort: string; path: string; query: string } | null {
  try {
    const url = new URL(value.trim());
    const defaults: Record<string, string> = { 'http:': '80', 'https:': '443' };
    return {
      scheme: url.protocol,
      host: url.hostname.toLowerCase(),
      port: url.port || defaults[url.protocol] || '',
      explicitPort: url.port,
      path: url.pathname.replace(/\/+$/, '').toLowerCase(),
      query: url.search,
    };
  } catch {
    return null;
  }
}

/**
 * Whether a stored key may stay with a changed address: the same destination
 * (ignoring case and a trailing slash), or the same host and path moved from
 * HTTP to HTTPS. The core applies the same rule when saving.
 */
export function keepsAddress(saved: string | null | undefined, edited: string | null | undefined): boolean {
  const a = (saved ?? '').trim();
  const b = (edited ?? '').trim();
  const pa = addressParts(a);
  const pb = addressParts(b);
  if (!pa || !pb) return a.replace(/\/+$/, '').toLowerCase() === b.replace(/\/+$/, '').toLowerCase();
  const same = pa.scheme === pb.scheme && pa.host === pb.host && pa.port === pb.port && pa.path === pb.path && pa.query === pb.query;
  const upgrade = pa.scheme === 'http:' && pb.scheme === 'https:' && pa.host === pb.host
    && pa.explicitPort === pb.explicitPort && pa.path === pb.path && pa.query === pb.query;
  return same || upgrade;
}

/**
 * True when the key has to be entered again: a key is stored, the address it
 * was saved for is being changed, and no new key was typed. Nothing is sent
 * until then; the core refuses such a save as well.
 */
export function keyRequiredForAddress(
  savedAddress: string | null | undefined,
  editedAddress: string | null | undefined,
  status: SecretStatus | null | undefined,
  typed: string | null | undefined,
): boolean {
  if (!status?.stored || newKeyPayload(typed) !== null) return false;
  if (!(editedAddress ?? '').trim()) return false;
  return !keepsAddress(savedAddress, editedAddress);
}

/** Whether a request can be made with a key: one typed now, or one stored. */
export function hasUsableKey(typed: string | null | undefined, status: SecretStatus | null | undefined): boolean {
  return newKeyPayload(typed) !== null || !!status?.stored;
}

/**
 * Removing the key of the provider that is in use breaks it, so that removal
 * asks for confirmation. Removing the key of any other provider does not.
 */
export function removalNeedsConfirmation(provider: string, activeProvider: string | null | undefined): boolean {
  return !!activeProvider && provider === activeProvider;
}

/** Field names that carry a secret. They are dropped from anything read from the core. */
const SECRET_FIELDS = ['apiKey', 'api_key', 'bearerToken', 'bearer_token', 'pat', 'token', 'accessToken', 'refreshToken', 'password', 'customOpenAIApiKey'];

/**
 * A copy of a value read from the core without any secret field. The core does
 * not send them; this keeps a mistake there from reaching the screen, a log or
 * an event.
 */
export function withoutSecrets<T>(value: T): T {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return value;
  const copy: Record<string, unknown> = { ...(value as Record<string, unknown>) };
  for (const field of SECRET_FIELDS) delete copy[field];
  return copy as T;
}

/**
 * The shared model configuration with a provider's refreshed key status
 * applied: for the provider in use (so a removal shows at once), and, for the
 * custom provider, in its own field as well.
 */
export function withRefreshedKeyStatus<T extends { provider: string; apiKeyStatus?: SecretStatus; customOpenAIKeyStatus?: SecretStatus }>(
  config: T,
  provider: string,
  status: SecretStatus,
): T {
  const active = config.provider === provider;
  if (provider === 'custom-openai') {
    return { ...config, customOpenAIKeyStatus: status, ...(active ? { apiKeyStatus: status } : {}) };
  }
  return active ? { ...config, apiKeyStatus: status } : config;
}
