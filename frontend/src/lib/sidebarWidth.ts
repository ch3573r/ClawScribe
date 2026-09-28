export const DEFAULT_SIDEBAR_WIDTH = 280;
export const MIN = 224;
export const MAX = 448;
const STORAGE_KEY = 'clawscribe.sidebarWidth';

export function clampSidebarWidth(px: number, viewportWidth: number): number {
  const width = Number.isFinite(px) ? px : DEFAULT_SIDEBAR_WIDTH;
  const cap = Number.isFinite(viewportWidth) ? Math.max(0, viewportWidth * 0.4) : MAX;
  return Math.min(Math.max(width, MIN), MAX, cap);
}

export function readStoredSidebarWidth(): number {
  try {
    const stored = window.localStorage.getItem(STORAGE_KEY);
    if (!stored?.trim()) return DEFAULT_SIDEBAR_WIDTH;
    const width = Number(stored);
    return Number.isFinite(width) && width >= MIN && width <= MAX ? width : DEFAULT_SIDEBAR_WIDTH;
  } catch {
    return DEFAULT_SIDEBAR_WIDTH;
  }
}

export function storeSidebarWidth(px: number): number {
  try {
    const width = clampSidebarWidth(px, Infinity);
    window.localStorage.setItem(STORAGE_KEY, String(width));
    return width;
  } catch {
    return DEFAULT_SIDEBAR_WIDTH;
  }
}
