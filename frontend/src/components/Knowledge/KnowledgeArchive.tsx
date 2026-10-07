"use client";
import { useMemo, useState } from "react";
import { useConfig } from "@/contexts/ConfigContext";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { libraryScope } from "@/lib/knowledge-state";
import { useKnowledgeSearch } from "@/hooks/useKnowledgeSearch";
import { SearchResults } from "./SearchResults";
import { KnowledgeChat } from "./KnowledgeChat";
import { EvidencePreview } from "./EvidencePreview";
import type { ProjectFilter } from "@/lib/library";
import type { SearchMode } from "@/types/knowledge";
export function KnowledgeArchive({
  meetings,
  projectFilter,
}: {
  meetings: { id: string; title: string }[];
  projectFilter: ProjectFilter;
}) {
  const { modelConfig } = useConfig();
  const [query, setQuery] = useState("");
  const [mode, setMode] = useState<SearchMode>("keyword");
  const [all, setAll] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  const [meetingQuery, setMeetingQuery] = useState("");
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const scope = useMemo(
    () => libraryScope(selected, all, projectFilter, from, to),
    [selected, all, projectFilter, from, to],
  );
  const state = useKnowledgeSearch(scope, null, {
    provider: modelConfig?.provider,
    model: modelConfig?.model,
  });
  const selectedCount = selected.length;
  const matchingMeetings = meetings.filter((meeting) => meeting.title.toLocaleLowerCase().includes(meetingQuery.trim().toLocaleLowerCase()));
  return (
    <section className="space-y-4" aria-label="Meeting memory">
      <div className="space-y-4">
        <div>
          <p className="text-sm text-muted-foreground">
            Choose meetings, then search transcripts or ask a question with cited sources.
          </p>
        </div>
        <fieldset className="space-y-3">
          <legend className="mb-2 text-sm font-medium">Sources</legend>
          <div className="flex flex-wrap items-center gap-3">
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={all}
              onChange={(e) => setAll(e.target.checked)}
              className="accent-primary"
            />
            Search all saved meetings
          </label>
          {!all && (
            <Popover>
              <PopoverTrigger asChild>
                <Button variant="outline" size="sm">Choose meetings{selectedCount ? ` (${selectedCount})` : ""}</Button>
              </PopoverTrigger>
              <PopoverContent align="start" className="w-80 max-w-[calc(100vw-3rem)] space-y-3">
                <input aria-label="Find meetings" value={meetingQuery} onChange={(event) => setMeetingQuery(event.target.value)}
                  placeholder="Find a meeting…" className="w-full rounded-md border border-input bg-background px-3 py-2 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" />
              <div className="max-h-56 space-y-2 overflow-y-auto">
                {matchingMeetings.map((meeting) => (
                  <label
                    key={meeting.id}
                    className="flex items-start gap-2 text-sm"
                  >
                    <input
                      type="checkbox"
                      checked={selected.includes(meeting.id)}
                      onChange={(e) =>
                        setSelected((ids) =>
                          e.target.checked
                            ? [...ids, meeting.id]
                            : ids.filter((id) => id !== meeting.id),
                        )
                      }
                      className="mt-1 accent-primary"
                    />
                    <span className="break-words">{meeting.title}</span>
                  </label>
                ))}
                {!matchingMeetings.length && (
                  <p className="text-xs text-muted-foreground">
                    No meetings match this search and project filter.
                  </p>
                )}
              </div>
              </PopoverContent>
            </Popover>
          )}
          <details>
            <summary className="cursor-pointer rounded text-sm text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">Date range{from || to ? " · active" : ""}</summary>
            <div className="mt-3 flex flex-wrap gap-3">
              <label className="text-xs">From (inclusive)<input type="date" value={from} onChange={(e) => setFrom(e.target.value)} className="mt-1 block rounded border border-input bg-background p-2 text-sm" /></label>
              <label className="text-xs">To (inclusive)<input type="date" value={to} onChange={(e) => setTo(e.target.value)} className="mt-1 block rounded border border-input bg-background p-2 text-sm" /></label>
            </div>
          </details>
          </div>
          {!all && selectedCount > 0 && <div className="flex flex-wrap gap-2" aria-label="Selected meetings">
            {selected.map((id) => {
              const title = meetings.find((meeting) => meeting.id === id)?.title ?? "Selected meeting outside project filter";
              return <button key={id} type="button" aria-label={`Remove ${title}`} onClick={() => setSelected((ids) => ids.filter((value) => value !== id))}
                className="max-w-full rounded-md border border-primary/25 bg-primary/10 px-2 py-1 text-xs text-foreground hover:bg-primary/20 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"><span className="block truncate">{title} · Remove</span></button>;
            })}
          </div>}
          <p className="text-xs text-muted-foreground">
            {projectFilter.untagged
              ? "Untagged meetings"
              : projectFilter.tags.length
                ? `${projectFilter.mode === "all" ? "All" : "Any"} projects: ${projectFilter.tags.join(", ")}`
                : "All projects"}{" "}
            ·{" "}
            {all ? "All saved meetings" : `${selectedCount} selected meetings`}.
            {!all && !selectedCount ? "Choose at least one meeting to begin." : ""}
          </p>
        </fieldset>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            void state.search(query, mode);
          }}
          className="flex flex-wrap items-end gap-3"
        >
          <label className="min-w-40 flex-1 text-xs font-medium">
            Transcript query
            <input
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                state.cancel();
              }}
              placeholder="Search what was said…"
              className="mt-1 block w-full rounded-md border border-input bg-background p-3 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            />
          </label>
          <label className="text-xs font-medium">
            Search mode
            <select
              value={mode}
              onChange={(e) => {
                setMode(e.target.value as SearchMode);
                state.cancel();
              }}
              className="mt-1 block rounded-md border border-input bg-background p-3 text-sm"
            >
              <option value="keyword">Keyword</option>
              <option value="hybrid">Semantic + keyword</option>
            </select>
          </label>
          <Button disabled={state.loading || !query.trim()} type="submit">
            Search transcripts
          </Button>
        </form>
        <p className="text-xs text-muted-foreground">
          Keyword search works without a model download. Semantic search can
          fall back to keyword; the result shows the mode actually used.
        </p>
        {from && to && from > to && (
          <p role="alert" className="text-sm text-destructive">
            The start date must be on or before the end date.
          </p>
        )}
        {state.error && !state.preview && (
          <p role="alert" className="text-sm text-destructive">
            {state.error}
          </p>
        )}
      </div>
      <div className="grid items-start gap-4 2xl:grid-cols-2">
        <SearchResults state={state} />
        <KnowledgeChat
          state={state}
          mode={mode}
          provider={modelConfig?.provider}
          model={modelConfig?.model}
        />
      </div>
      <EvidencePreview state={state} />
    </section>
  );
}
