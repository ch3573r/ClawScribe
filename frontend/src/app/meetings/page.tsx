"use client";

import { useCallback, useMemo, useState } from "react";
import {
  ArrowRight,
  Clock3,
  FileText,
  NotebookPen,
  RefreshCw,
  Search,
  X,
} from "lucide-react";
import { useRouter } from "next/navigation";
import { useSidebar } from "@/components/Sidebar/SidebarProvider";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "@/components/ui/input-group";

export default function MeetingsPage() {
  const router = useRouter();
  const {
    meetings,
    setCurrentMeeting,
    searchTranscripts,
    searchResults,
    isSearching,
    refetchMeetings,
  } = useSidebar();
  const [query, setQuery] = useState("");
  const [isRefreshing, setIsRefreshing] = useState(false);

  const normalizedQuery = query.trim().toLowerCase();

  const titleMatches = useMemo(() => {
    if (!normalizedQuery) return meetings;
    return meetings.filter((meeting) =>
      meeting.title.toLowerCase().includes(normalizedQuery),
    );
  }, [meetings, normalizedQuery]);

  const hasTranscriptMatches = normalizedQuery.length > 0 && searchResults.length > 0;
  const visibleMeetings = normalizedQuery ? titleMatches : meetings;

  const handleQueryChange = useCallback(
    (value: string) => {
      setQuery(value);
      void searchTranscripts(value);
    },
    [searchTranscripts],
  );

  const handleRefresh = useCallback(async () => {
    setIsRefreshing(true);
    try {
      await refetchMeetings();
    } finally {
      setIsRefreshing(false);
    }
  }, [refetchMeetings]);

  const openMeeting = useCallback(
    (id: string, title: string) => {
      setCurrentMeeting({ id, title });
      router.push(`/meeting-details?id=${encodeURIComponent(id)}`);
    },
    [router, setCurrentMeeting],
  );

  const titleFor = useCallback(
    (id: string, fallback: string) =>
      meetings.find((meeting) => meeting.id === id)?.title || fallback,
    [meetings],
  );

  const shownCount = hasTranscriptMatches
    ? searchResults.length
    : visibleMeetings.length;

  return (
    <div className="min-h-0 flex-1 overflow-y-auto bg-background text-foreground">
      <div className="mx-auto flex min-h-full w-full max-w-[1600px] flex-col gap-6 px-8 py-7">
        <header className="flex flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
          <div>
            <h1 className="text-4xl font-semibold tracking-tight text-foreground">
              Meetings
            </h1>
            <p className="mt-2 max-w-2xl text-base text-muted-foreground">
              Browse saved recordings, transcripts, and generated summaries.
            </p>
          </div>

          <button
            onClick={handleRefresh}
            className="inline-flex w-fit items-center gap-2 rounded-md border border-border bg-card px-4 py-2 text-sm font-medium text-foreground shadow-sm transition hover:bg-muted"
          >
            <RefreshCw
              className={`h-4 w-4 ${isRefreshing ? "animate-spin" : ""}`}
            />
            Refresh
          </button>
        </header>

        <section className="rounded-lg border border-border bg-card p-5 shadow-sm">
          <div className="grid gap-4 xl:grid-cols-[1fr_auto] xl:items-center">
            <InputGroup className="rounded-md border-border bg-background text-foreground shadow-none">
              <InputGroupInput
                id="meetings-search"
                placeholder="Search meetings and transcripts..."
                value={query}
                onChange={(event) => handleQueryChange(event.target.value)}
                className="placeholder:text-muted-foreground"
              />
              <InputGroupAddon>
                <Search className="h-4 w-4 text-muted-foreground" />
              </InputGroupAddon>
              {query && (
                <InputGroupAddon align="inline-end">
                  <InputGroupButton onClick={() => handleQueryChange("")}>
                    <X className="h-4 w-4" />
                  </InputGroupButton>
                </InputGroupAddon>
              )}
            </InputGroup>

            <div className="flex flex-wrap items-center gap-3 text-sm text-muted-foreground">
              <span>{meetings.length} total</span>
              <span className="h-1 w-1 rounded-full bg-muted-foreground/40" />
              <span>{shownCount} shown</span>
              {isSearching && (
                <>
                  <span className="h-1 w-1 rounded-full bg-muted-foreground/40" />
                  <span className="text-primary">Searching...</span>
                </>
              )}
            </div>
          </div>
        </section>

        <div className="grid min-h-0 gap-5 xl:grid-cols-[minmax(0,1fr)_20rem]">
          <section className="min-w-0 rounded-lg border border-border bg-card shadow-sm">
            <div className="flex items-center justify-between gap-4 border-b border-border px-6 py-5">
              <div>
                <h2 className="text-lg font-semibold text-foreground">
                  {hasTranscriptMatches ? "Transcript Matches" : "Saved Meetings"}
                </h2>
                <p className="mt-1 text-sm text-muted-foreground">
                  {normalizedQuery
                    ? `Results for "${query.trim()}"`
                    : "Your meeting archive"}
                </p>
              </div>
              <NotebookPen className="h-5 w-5 text-primary" />
            </div>

            {hasTranscriptMatches ? (
              <div className="divide-y divide-border">
                {searchResults.map((result, index) => {
                  const title = titleFor(result.id, result.title);
                  return (
                    <button
                      key={`${result.id}-${index}`}
                      onClick={() => openMeeting(result.id, title)}
                      className="group grid w-full gap-4 px-6 py-5 text-left transition hover:bg-muted lg:grid-cols-[minmax(0,1fr)_auto] lg:items-center"
                    >
                      <div className="flex min-w-0 gap-3">
                        <span className="mt-0.5 flex h-10 w-10 shrink-0 items-center justify-center rounded-md border border-border bg-muted text-primary">
                          <FileText className="h-4 w-4" />
                        </span>
                        <div className="min-w-0">
                          <h3 className="truncate text-sm font-semibold text-foreground">
                            {title}
                          </h3>
                          <div className="mt-1 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                            <span>Transcript match</span>
                            {result.timestamp ? (
                              <>
                                <span className="h-1 w-1 rounded-full bg-muted-foreground/40" />
                                <span>{result.timestamp}</span>
                              </>
                            ) : null}
                          </div>
                          <p className="mt-3 line-clamp-2 text-sm leading-6 text-muted-foreground">
                            {result.matchContext}
                          </p>
                        </div>
                      </div>
                      <span className="inline-flex items-center gap-2 text-sm font-medium text-primary lg:justify-self-end">
                        Open details
                        <ArrowRight className="h-4 w-4 transition group-hover:translate-x-0.5" />
                      </span>
                    </button>
                  );
                })}
              </div>
            ) : visibleMeetings.length > 0 ? (
              <div className="divide-y divide-border">
                {visibleMeetings.map((meeting) => (
                  <button
                    key={meeting.id}
                    onClick={() => openMeeting(meeting.id, meeting.title)}
                    className="group grid w-full gap-4 px-6 py-5 text-left transition hover:bg-muted lg:grid-cols-[minmax(0,1fr)_auto] lg:items-center"
                  >
                    <div className="flex min-w-0 items-center gap-3">
                      <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-md border border-border bg-muted text-primary">
                        <FileText className="h-4 w-4" />
                      </span>
                      <div className="min-w-0">
                        <h3 className="truncate text-sm font-semibold text-foreground">
                          {meeting.title}
                        </h3>
                        <div className="mt-1 flex items-center gap-2 text-xs text-muted-foreground">
                          <Clock3 className="h-3.5 w-3.5" />
                          Saved meeting
                        </div>
                      </div>
                    </div>
                    <span className="inline-flex items-center gap-2 text-sm font-medium text-muted-foreground transition group-hover:text-primary lg:justify-self-end">
                      Open details
                      <ArrowRight className="h-4 w-4 transition group-hover:translate-x-0.5" />
                    </span>
                  </button>
                ))}
              </div>
            ) : (
              <div className="flex flex-col items-center justify-center px-6 py-16 text-center">
                <span className="flex h-12 w-12 items-center justify-center rounded-md border border-border bg-muted text-primary">
                  <FileText className="h-5 w-5" />
                </span>
                <h3 className="mt-4 text-base font-semibold text-foreground">
                  No meetings found
                </h3>
                <p className="mt-2 max-w-sm text-sm leading-6 text-muted-foreground">
                  Try another search or start a new recording from Home.
                </p>
              </div>
            )}
          </section>

          <aside className="space-y-5">
            <section className="rounded-lg border border-border bg-card p-5 shadow-sm">
              <h2 className="text-base font-semibold text-foreground">
                Archive
              </h2>
              <div className="mt-4 divide-y divide-border text-sm">
                <div className="flex items-center justify-between py-3">
                  <span className="text-muted-foreground">Saved meetings</span>
                  <span className="font-semibold text-foreground">
                    {meetings.length}
                  </span>
                </div>
                <div className="flex items-center justify-between py-3">
                  <span className="text-muted-foreground">Current results</span>
                  <span className="font-semibold text-foreground">
                    {shownCount}
                  </span>
                </div>
                <div className="flex items-center justify-between py-3">
                  <span className="text-muted-foreground">Transcript hits</span>
                  <span className="font-semibold text-foreground">
                    {normalizedQuery ? searchResults.length : 0}
                  </span>
                </div>
              </div>
            </section>
          </aside>
        </div>
      </div>
    </div>
  );
}
