"use client";

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { createLatestSearch } from "@/lib/latest-search";
import type { EvidenceRef, KnowledgeScope } from "@/types/knowledge";

export const MAX_LIBRARY_QUERY = 256;

interface TitleHit { id: string; title: string; date: string }
export interface TranscriptHit {
  meeting_id: string;
  transcript_id: string;
  title: string;
  date: string;
  text: string;
  speaker: string | null;
  start_seconds: number | null;
  evidence: EvidenceRef;
}
interface LibraryResults { titles: TitleHit[]; passages: TranscriptHit[]; limited: boolean }

export function useLibrarySearch(query: string, active: boolean, scope?: KnowledgeScope, onePerMeeting = false, revision?: unknown) {
  const text = query.trim();
  const scopeKey = JSON.stringify(scope ?? null);
  const valid = Array.from(text).length <= MAX_LIBRARY_QUERY;
  const [search, setSearch] = useState<{text: string; scopeKey: string; results: LibraryResults | null; error: string} | null>(null);

  useEffect(() => {
    setSearch(null);
    if (!active || !text || !valid) return;
    let error = "";
    const job = createLatestSearch<LibraryResults | null>({
      run: query => invoke<LibraryResults>("api_search_library", { query, scope: JSON.parse(scopeKey), onePerMeeting }),
      empty: null,
      onSearching: () => {},
      onResults: results => setSearch({ text, scopeKey, results, error }),
      onError: () => { error = "Could not search your saved meetings. Try again."; },
    });
    void job.search(text);
    return job.cancel;
  }, [text, active, scopeKey, onePerMeeting, valid, revision]);

  const current = active && search?.text === text && search.scopeKey === scopeKey ? search : null;
  return {
    results: current?.results ?? null,
    searching: active && !!text && valid && !current,
    error: active && !valid ? `Search using up to ${MAX_LIBRARY_QUERY} characters.` : current?.error ?? "",
  };
}
