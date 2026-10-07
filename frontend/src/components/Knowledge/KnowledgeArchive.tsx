"use client";
import { useMemo, useState } from "react";
import { useConfig } from "@/contexts/ConfigContext";
import { Button } from "@/components/ui/button";
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
  return (
    <section className="space-y-4" aria-label="Meeting memory">
      <div className="space-y-4 rounded-lg border border-border bg-card p-5 shadow-sm">
        <div>
          <h2 className="text-lg font-semibold">Meeting memory</h2>
          <p className="mt-1 text-sm text-muted-foreground">
            Search transcript passages and ask cited questions across a
            deliberate selection of saved meetings.
          </p>
        </div>
        <fieldset className="space-y-3">
          <legend className="mb-2 text-sm font-medium">Source selection</legend>
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
            <details className="rounded-md border border-border p-3">
              <summary className="cursor-pointer text-sm focus-visible:ring-2 focus-visible:ring-ring">
                {selectedCount} meetings selected
              </summary>
              <div className="mt-3 max-h-48 space-y-2 overflow-y-auto">
                {meetings.map((meeting) => (
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
                {!meetings.length && (
                  <p className="text-xs text-muted-foreground">
                    No meetings match the project filter.
                  </p>
                )}
              </div>
            </details>
          )}
          <p className="text-xs text-muted-foreground">
            {projectFilter.untagged
              ? "Untagged meetings"
              : projectFilter.tags.length
                ? `${projectFilter.mode === "all" ? "All" : "Any"} projects: ${projectFilter.tags.join(", ")}`
                : "All projects"}{" "}
            ·{" "}
            {all ? "All saved meetings" : `${selectedCount} selected meetings`}.
            An empty selection searches no meetings.
          </p>
          <div className="flex flex-wrap gap-3">
            <label className="text-xs">
              From (inclusive)
              <input
                type="date"
                value={from}
                onChange={(e) => setFrom(e.target.value)}
                className="mt-1 block rounded border border-input bg-background p-2 text-sm"
              />
            </label>
            <label className="text-xs">
              To (inclusive)
              <input
                type="date"
                value={to}
                onChange={(e) => setTo(e.target.value)}
                className="mt-1 block rounded border border-input bg-background p-2 text-sm"
              />
            </label>
          </div>
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
