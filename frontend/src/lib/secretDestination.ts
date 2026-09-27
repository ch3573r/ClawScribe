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
        (a === 192 && b === 168) || (a === 169 && b === 254);
    }
    return !host.includes('.') || ['.local', '.lan', '.internal', '.home.arpa'].some(suffix => host.endsWith(suffix));
  } catch { return false; }
}
