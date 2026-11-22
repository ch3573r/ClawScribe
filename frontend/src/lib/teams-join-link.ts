export function teamsJoinLink(value: unknown): string | null {
  if (typeof value !== 'string' || value.length > 4096 || !/^https:\/\/[^/]/i.test(value) ||
    /[\s\\\u0000-\u001f\u007f]/.test(value)) return null;
  try {
    const url = new URL(value);
    const path = url.pathname.replace(/\/+$/, '');
    const thread = /^\/l\/meetup-join\/([^/]+)\/0$/.exec(path)?.[1];
    const validPath = /^\/meet\/\d+$/.test(path) ||
      (thread !== undefined && /^[a-zA-Z0-9_.:@-]+$/.test(decodeURIComponent(thread)));
    if (url.protocol === 'https:' && !url.username && !url.password && !url.port &&
      ['teams.microsoft.com', 'teams.cloud.microsoft', 'teams.live.com'].includes(url.hostname) &&
      validPath) {
      url.hash = '';
      return url.href;
    }
  } catch {}
  return null;
}
