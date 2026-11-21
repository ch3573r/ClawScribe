export function teamsJoinLink(value: unknown): string | null {
  if (typeof value !== 'string' || value.length > 4096 || /[\s\\\u0000-\u001f]/.test(value)) return null;
  try {
    const url = new URL(value);
    if (url.protocol === 'https:' && !url.username && !url.password && !url.port &&
      ['teams.microsoft.com', 'teams.cloud.microsoft', 'teams.live.com'].includes(url.hostname) &&
      /^\/(?:meet\/\d+|l\/meetup-join\/[a-zA-Z0-9_%:.@-]+\/0)\/*$/.test(url.pathname)) {
      url.hash = '';
      return url.href;
    }
  } catch {}
  return null;
}
