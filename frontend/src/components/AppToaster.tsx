'use client'

import { useEffect, useState } from 'react'
import { Toaster } from 'sonner'
import {
  getStoredThemePreference,
  resolveThemePreference,
  subscribeToSystemTheme,
  type ResolvedTheme,
} from '@/lib/theme'

/** The resolved theme the app has applied to the page, or the stored preference before it has. */
function currentTheme(): ResolvedTheme {
  const applied = document.documentElement.dataset.theme
  if (applied === 'light' || applied === 'dark') return applied
  return resolveThemePreference(getStoredThemePreference())
}

/**
 * Notifications in the bottom-right corner, in the theme the app is showing.
 * Bottom-right clears the question field, the summary context bar and the
 * other inputs that sit along the bottom edge of a page.
 */
export function AppToaster() {
  const [theme, setTheme] = useState<ResolvedTheme>('dark')

  useEffect(() => {
    const sync = () => setTheme(currentTheme())
    sync()
    // Settings changes the theme by rewriting these attributes; the system
    // theme can change underneath a "system" preference.
    const observer = new MutationObserver(sync)
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme', 'class'] })
    const unsubscribe = subscribeToSystemTheme(sync)
    return () => {
      observer.disconnect()
      unsubscribe()
    }
  }, [])

  return <Toaster position="bottom-right" theme={theme} richColors closeButton />
}
