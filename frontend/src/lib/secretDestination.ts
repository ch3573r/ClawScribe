// Display only: the backend enforces this policy and validates DNS before sending.
export function needsHttpOptIn(value: string): boolean {
  try {
    const url = new globalThis.URL(value);
    if (url.protocol !== 'http:') return false;
    const host = url.hostname;
    if (host === 'localhost' || host === '[::1]') return false;
    if (host.startsWith('[')) return /^\[(?:f[cd][0-9a-f]{2}:|fe[89ab][0-9a-f]:)/i.test(host);
    if (/^\d+\.\d+\.\d+\.\d+$/.test(host)) {
      const [a, b] = host.split('.').map(Number);
      return a === 10 || (a === 172 && b >= 16 && b <= 31) ||
        (a === 192 && b === 168) || (a === 169 && b === 254) ||
        (a === 100 && b >= 64 && b <= 127);
    }
    return !host.includes('.') || ['.local', '.lan', '.internal', '.home.arpa', '.ts.net'].some(suffix => host.endsWith(suffix));
  } catch { return false; }
}

// Settings stay editable even when a previously saved URL fails today's policy.
export function secretDestinationError(value: string, allowUnencrypted: boolean): string | null {
  if (!value.trim()) return null;
  try {
    const url = new globalThis.URL(value);
    if (!url.hostname || url.username || url.password) return 'Use an endpoint with a host and without credentials in its URL.';
    if (url.protocol === 'https:') return null;
    if (url.protocol !== 'http:') return 'The endpoint must use HTTP or HTTPS.';
    if (url.hostname === 'localhost' || url.hostname === '[::1]' || /^127\./.test(url.hostname)) return null;
    if (!needsHttpOptIn(value)) return 'Public HTTP endpoints are not allowed. Change the endpoint to HTTPS.';
    return allowUnencrypted ? null : 'Enable the local-network HTTP option below or use HTTPS.';
  } catch { return 'Enter a valid endpoint URL.'; }
}
