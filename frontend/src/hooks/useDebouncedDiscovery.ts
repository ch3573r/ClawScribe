"use client";

import { useEffect, useRef, useState } from "react";
import { createLatestSearch } from "@/lib/latest-search";
import { createSearchLane } from "@/lib/search-lane";

export function useDebouncedDiscovery<T>(
  query: string,
  active: boolean,
  run: (query: string) => Promise<T>,
  revision?: unknown,
) {
  const text = query.trim();
  const lane = useRef<ReturnType<typeof createSearchLane<T>> | null>(null);
  lane.current ??= createSearchLane<T>();
  const [settled, setSettled] = useState<{ text: string; run: typeof run; revision: unknown; data: T | null; error: boolean } | null>(null);

  useEffect(() => {
    setSettled(null);
    if (!active || !text) return;
    const controller = new AbortController();
    const schedule = lane.current!;
    let error = false;
    const search = createLatestSearch<T | null>({
      empty: null,
      run: query => schedule(controller.signal, () => run(query)),
      onResults: data => setSettled({ text, run, revision, data, error }),
      onSearching: () => {},
      onError: () => { error = true; },
    });
    void search.search(text);
    return () => { controller.abort(); search.cancel(); };
  }, [active, text, run, revision]);

  const current = active && settled?.text === text && settled.run === run && settled.revision === revision ? settled : null;
  return { data: current?.data ?? null, loading: active && !!text && !current, error: current?.error ?? false };
}
