"use client";
import { useState } from "react";
import { Checkbox } from "@/components/ui/checkbox";
import type { SavedHit } from "@/lib/archive-search";

/** Only checked, still-visible source revisions are offered to the composer. */
export function useResultNotes(view: string) {
  const [chosen, setChosen] = useState<{ view: string; keys: string[] }>({ view, keys: [] });
  if (view !== chosen.view) setChosen({ view, keys: [] });
  const keys = chosen.view === view ? chosen.keys : [];
  return {
    references: (hits: SavedHit[]) => hits.filter(hit => hit.reference.source !== "summary" && keys.includes(hit.reference.fingerprint)).map(hit => hit.reference),
    control(hit: SavedHit) {
      if (hit.reference.source === "summary") return null;
      const key = hit.reference.fingerprint;
      return <label className="flex items-center gap-2 text-xs">
        <Checkbox checked={keys.includes(key)} disabled={keys.length >= 8 && !keys.includes(key)} onCheckedChange={checked =>
          setChosen(current => {
            const previous = current.view === view ? current.keys : [];
            return { view, keys: checked === true ? [...new Set([...previous, key])].slice(0, 8) : previous.filter(value => value !== key) };
          })} />
        Review excerpt with a question
      </label>;
    },
  };
}
