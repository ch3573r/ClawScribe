"use client";

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createLatestSearch } from "@/lib/latest-search";
import type { EvidenceRef, KnowledgeScope } from "@/types/knowledge";
import type { SavedHit, SearchExcerpt } from "@/lib/archive-search";

export const MAX_LIBRARY_QUERY = 256;

interface TitleHit { id: string; title: string; date: string; highlights?: SearchExcerpt['highlights'] }
export interface TranscriptHit extends SearchExcerpt {
  meeting_id: string;
  transcript_id: string;
  title: string;
  date: string;
  speaker: string | null;
  start_seconds: number | null;
  evidence: EvidenceRef;
}
interface LibraryResults { titles: TitleHit[]; passages: TranscriptHit[]; saved: SavedHit[]; limited: boolean }

export function useLibrarySearch(query: string, active: boolean, scope?: KnowledgeScope, onePerMeeting = false, revision?: unknown) {
  const text = query.trim();
  const scopeKey = JSON.stringify(scope ?? null);
  const valid = Array.from(text).length <= MAX_LIBRARY_QUERY;
  const watch = active && !!text && valid;
  const [sourceRevision, setSourceRevision] = useState(0);
  useEffect(() => {
    if (!watch) return;
    let current = true;
    const subs = ["summary-job-changed", "transcript-text-edited", "retranscription-complete"].map(event =>
      listen(event, () => { if (current) setSourceRevision(value => value + 1); }));
    subs.forEach(sub => void sub.catch(() => {}));
    return () => { current = false; subs.forEach(sub => void sub.then(off => off()).catch(() => {})); };
  }, [watch]);
  const [search, setSearch] = useState<{text: string; scopeKey: string; results: LibraryResults | null; error: string} | null>(null);

  useEffect(() => {
    setSearch(null);
    if (!active || !text || !valid) return;
    let error = "";
    const job = createLatestSearch<LibraryResults | null>({
      run: query => invoke<LibraryResults>("search_saved_meetings", { query, scope: JSON.parse(scopeKey), onePerMeeting }),
      empty: null,
      onSearching: () => {},
      onResults: results => setSearch({ text, scopeKey, results, error }),
      onError: () => { error = "Could not search your saved meetings. Try again."; },
    });
    void job.search(text);
    return job.cancel;
  }, [text, active, scopeKey, onePerMeeting, valid, revision, sourceRevision]);

  const current = active && search?.text === text && search.scopeKey === scopeKey ? search : null;
  return {
    results: current?.results ?? null,
    searching: active && !!text && valid && !current,
    error: active && !valid ? `Search using up to ${MAX_LIBRARY_QUERY} characters.` : current?.error ?? "",
  };
}
