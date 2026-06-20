"use client";

import { useEffect } from "react";
import { useRouter } from "next/navigation";

/**
 * In-app keyboard shortcuts — only while ClawScribe is focused. (Global
 * shortcuts that work when the app is in the background are handled by the Tauri
 * layer in shortcuts.rs and configured under Settings → Shortcuts.)
 *
 *   Ctrl/⌘ + ,   open Settings
 *   Ctrl/⌘ + K   focus the meeting search
 *   Ctrl/⌘ + G   generate / regenerate the summary (Meeting Notes listens)
 *
 * K and G stand down while you're typing in an input or the summary editor, so
 * they never steal the editor's own shortcuts (e.g. BlockNote's Ctrl+K link).
 */
export function AppShortcuts() {
  const router = useRouter();

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.ctrlKey || e.metaKey;
      if (!mod || e.altKey) return;
      const key = e.key.toLowerCase();

      // Settings is harmless nav and "," isn't an editor key, so allow it
      // anywhere.
      if (key === ",") {
        e.preventDefault();
        router.push("/settings");
        return;
      }

      const t = e.target as HTMLElement | null;
      const typing =
        !!t &&
        (t.tagName === "INPUT" ||
          t.tagName === "TEXTAREA" ||
          t.isContentEditable);
      if (typing) return;

      if (key === "k") {
        e.preventDefault();
        const el = document.getElementById("meeting-search") as HTMLInputElement | null;
        el?.focus();
        el?.select();
      } else if (key === "g") {
        e.preventDefault();
        window.dispatchEvent(new CustomEvent("shortcut:generate-summary"));
      }
    };

    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [router]);

  return null;
}
