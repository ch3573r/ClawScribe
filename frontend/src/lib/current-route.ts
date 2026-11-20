export interface CurrentRoute {
  /** Normalised path: "/", "/settings", "/meeting-details". */
  path: string;
  /** The `id` query parameter, if any. */
  id: string | null;
}

/**
 * The page the user is on right now. Read it when it matters rather than from a
 * hook: a hook belongs to a component that may have unmounted by then.
 *
 * The app is a static export served by Tauri, and the client router uses the
 * History API, so the location is the real route. A freshly loaded window can
 * show it as `/index.html`, `/settings.html` or with a trailing slash, so those
 * forms are folded into the route name.
 */
export function currentRoute(location: Pick<Location, 'pathname' | 'search'> | undefined = typeof window === 'undefined' ? undefined : window.location): CurrentRoute {
  if (!location) return { path: '/', id: null };
  let path = location.pathname.replace(/[.]html$/, '').replace(/\/+$/, '');
  if (path === '' || path === '/index') path = '/';
  return { path, id: new URLSearchParams(location.search).get('id') };
}
