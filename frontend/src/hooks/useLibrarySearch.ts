"use client";

import { useEffect, useRef, useState } from "react";
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
type SavedCursor = [string | null, string | null, string | null];
interface LibraryResults {
  titles: TitleHit[];
  passages: TranscriptHit[];
  saved: SavedHit[];
  limited: boolean;
  saved_next: SavedCursor | null;
}
type Search = { text: string; scopeKey: string; results: LibraryResults | null; error: string; moreError?: string };

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
  const [search, setSearch] = useState<Search | null>(null);
  const pendingMore = useRef<Search | null>(null);
  const [loadingPage, setLoadingPage] = useState<Search | null>(null);

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
  const loadMore = async () => {
    if (onePerMeeting || !current?.results?.saved_next || pendingMore.current === current) return;
    const previous = current.results;
    pendingMore.current = current;
    setLoadingPage(current);
    try {
      const page = await invoke<LibraryResults>("search_saved_meetings", {
        query: text, scope: JSON.parse(scopeKey), onePerMeeting, savedCursor: previous.saved_next,
      });
      setSearch(value => value === current ? {
        ...value, moreError: "",
        results: {
          ...previous,
          saved: [...previous.saved, ...page.saved],
          saved_next: page.saved_next,
          limited: previous.limited || page.limited,
        },
      } : value);
    } catch {
      setSearch(value => value === current ? { ...value, moreError: "Could not load more matches. Try again." } : value);
    } finally {
      if (pendingMore.current === current) pendingMore.current = null;
      setLoadingPage(value => value === current ? null : value);
    }
  };
  return {
    results: current?.results ?? null,
    searching: active && !!text && valid && !current,
    error: active && !valid ? `Search using up to ${MAX_LIBRARY_QUERY} characters.` : current?.error ?? "",
    loadMore,
    loadingMore: !!current && loadingPage === current,
    moreError: current?.moreError ?? "",
  };
}
