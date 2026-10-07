"use client";

import { useCallback, useMemo, useState } from "react";
import {
  ArrowRight,
  ChevronDown,
  Clock3,
  FileText,
  NotebookPen,
  RefreshCw,
  Search,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { PageSection } from "@/components/ui/page-section";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useRouter } from "next/navigation";
import { useSidebar } from "@/components/Sidebar/SidebarProvider";
import { LibraryBackup } from "@/components/LibraryBackup";
import { KnowledgeArchive } from "@/components/Knowledge/KnowledgeArchive";
import { matchesProjectFilter, type ProjectFilter } from "@/lib/library";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuCheckboxItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuItem,
} from "@/components/ui/dropdown-menu";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "@/components/ui/input-group";

type SortMode = "newest" | "oldest" | "title";

function timeValue(value?: string): number {
  if (!value) return 0;
  const time = Date.parse(value);
  return Number.isFinite(time) ? time : 0;
}

function formatMeetingDate(value?: string): string {
  if (!value) return "Saved meeting";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "Saved meeting";
  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(date);
}

export default function MeetingsPage() {
  const router = useRouter();
  const {
    meetings,
    setCurrentMeeting,
    refetchMeetings,
    projectTags,
    projectTagsError,
    projectTagsLoading,
    refreshProjectTags,
  } = useSidebar();
  const [query, setQuery] = useState("");
  const [tab, setTab] = useState<"archive" | "memory">("archive");
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [sortMode, setSortMode] = useState<SortMode>("newest");
  const [projectFilter, setProjectFilter] = useState<ProjectFilter>({ tags: [], untagged: false, mode: 'any' });
  const tagOptions = useMemo(() => [...new Map(projectTags.map(row => [row.tag.toLowerCase(), row.tag])).values()].sort((a, b) => a.localeCompare(b)), [projectTags]);
  const projectFilterLabel = projectFilter.untagged ? 'Untagged'
    : projectFilter.tags.length === 0 ? 'All projects'
    : projectFilter.tags.length === 1 ? projectFilter.tags[0]
    : `${projectFilter.tags.length} projects`;

  const normalizedQuery = query.trim().toLowerCase();

  const sortedMeetings = useMemo(() => {
    const next = meetings.filter(meeting => matchesProjectFilter(meeting.id, projectFilter, projectTags));
    next.sort((a, b) => {
      if (sortMode === "title") {
        return a.title.localeCompare(b.title, undefined, { sensitivity: "base" });
      }
      const aTime = timeValue(a.created_at);
      const bTime = timeValue(b.created_at);
      if (aTime !== bTime) {
        return sortMode === "oldest" ? aTime - bTime : bTime - aTime;
      }
      return a.title.localeCompare(b.title, undefined, { sensitivity: "base" });
    });
    return next;
  }, [meetings, sortMode, projectFilter, projectTags]);

  const titleMatches = useMemo(() => {
    if (!normalizedQuery) return sortedMeetings;
    return sortedMeetings.filter((meeting) =>
      meeting.title.toLowerCase().includes(normalizedQuery),
    );
  }, [normalizedQuery, sortedMeetings]);

  const visibleMeetings = normalizedQuery ? titleMatches : sortedMeetings;

  const handleQueryChange = useCallback(
    (value: string) => {
      setQuery(value);
    },
    [],
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

  const shownCount = visibleMeetings.length;

  const projectControl = <>
<div className="flex items-center gap-2">Project
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button variant="ghost"
                      type="button"
                      aria-label={`Project: ${projectFilterLabel}`}
                      className="flex h-9 max-w-48 items-center justify-between gap-2 rounded-md border border-input bg-background px-3 py-2 text-sm shadow-sm focus:outline-none focus:ring-1 focus:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
                      disabled={projectTagsLoading || !!projectTagsError}
                    >
                      <span className="truncate">{projectFilterLabel}</span>
                      <ChevronDown className="h-4 w-4 shrink-0 opacity-50" />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end" className="max-h-72 overflow-y-auto">
                    <DropdownMenuRadioGroup
                      aria-label="Project matching"
                      value={projectFilter.mode}
                      onValueChange={mode => setProjectFilter(current => ({ ...current, mode: mode as ProjectFilter['mode'] }))}
                    >
                      <DropdownMenuRadioItem value="any" disabled={projectFilter.tags.length < 2} onSelect={event => event.preventDefault()}>Match any</DropdownMenuRadioItem>
                      <DropdownMenuRadioItem value="all" disabled={projectFilter.tags.length < 2} onSelect={event => event.preventDefault()}>Match all</DropdownMenuRadioItem>
                    </DropdownMenuRadioGroup>
                    <DropdownMenuSeparator />
                    <DropdownMenuCheckboxItem
                      checked={projectFilter.untagged}
                      onSelect={event => event.preventDefault()}
                      onCheckedChange={checked => setProjectFilter(current => ({ ...current, tags: [], untagged: checked === true }))}
                    >Untagged</DropdownMenuCheckboxItem>
                    {tagOptions.map(tag => (
                      <DropdownMenuCheckboxItem
                        key={tag.toLowerCase()}
                        checked={projectFilter.tags.some(selected => selected.toLowerCase() === tag.toLowerCase())}
                        onSelect={event => event.preventDefault()}
                        onCheckedChange={checked => setProjectFilter(current => ({
                          ...current,
                          untagged: false,
                          tags: [...current.tags.filter(selected => selected.toLowerCase() !== tag.toLowerCase()), ...(checked === true ? [tag] : [])],
                        }))}
                      >{tag}</DropdownMenuCheckboxItem>
                    ))}
                    <DropdownMenuSeparator />
                    <DropdownMenuItem onSelect={() => setProjectFilter({ tags: [], untagged: false, mode: 'any' })}>Clear filter</DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>
              {projectTagsLoading && <span role="status">Loading project tags…</span>}
              {projectTagsError && <Button variant="ghost" className="text-destructive underline" onClick={() => { void refreshProjectTags(); }}>Retry loading tags</Button>}
  </>;

  return (
    <div className="h-full min-h-0 overflow-hidden bg-background text-foreground">
      <div className="mx-auto flex h-full min-h-0 w-full max-w-[1600px] flex-col gap-4 px-5 py-5">
        <header className="flex shrink-0 flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
          <div>
            <h1 className="text-3xl font-semibold tracking-tight text-foreground">
              Meetings
            </h1>
            <p className="mt-2 max-w-2xl text-base text-muted-foreground">
              Browse saved recordings, transcripts, and generated summaries.
            </p>
          </div>

          {tab === "archive" && <div className="flex flex-wrap items-center gap-2"><LibraryBackup /><Button variant="outline" disabled={isRefreshing}
            onClick={handleRefresh}
            className="inline-flex w-fit items-center gap-2 rounded-md border border-border bg-card px-4 py-2 text-sm font-medium text-foreground shadow-sm transition hover:bg-muted"
          >
            <RefreshCw
              className={`h-4 w-4 ${isRefreshing ? "animate-spin" : ""}`}
            />
            Refresh
          </Button>
          </div>}
        </header>

        <Tabs value={tab} onValueChange={value => setTab(value as "archive" | "memory")} className="flex min-h-0 flex-1 flex-col gap-4">
          <TabsList aria-label="Meetings views" className="w-fit shrink-0">
            <TabsTrigger value="archive" id="meetings-archive-tab" aria-controls="meetings-archive-panel">Archive</TabsTrigger>
            <TabsTrigger value="memory" id="meetings-memory-tab" aria-controls="meeting-memory-panel">Meeting memory</TabsTrigger>
          </TabsList>
          <TabsContent forceMount id="meeting-memory-panel" hidden={tab !== "memory"} className={tab === "memory" ? "m-0 min-h-0 flex-1" : "hidden"}>
            <KnowledgeArchive meetings={sortedMeetings} projectFilter={projectFilter} projectControl={projectControl} />
          </TabsContent>
          <TabsContent forceMount id="meetings-archive-panel" hidden={tab !== "archive"} className={tab === "archive" ? "m-0 min-h-0 flex-1 space-y-4 overflow-y-auto" : "hidden"}>
        <PageSection className="rounded-lg border border-border bg-card p-5 shadow-sm">
          <div className="grid gap-4 xl:grid-cols-[1fr_auto] xl:items-center">
            <div hidden={tab !== "archive"}><InputGroup className="rounded-md border-border bg-background text-foreground shadow-none">
              <InputGroupInput
                id="meetings-search"
                placeholder="Filter meeting titles..."
                value={query}
                onChange={(event) => handleQueryChange(event.target.value)}
                className="placeholder:text-muted-foreground"
              />
              <InputGroupAddon>
                <Search className="h-4 w-4 text-muted-foreground" />
              </InputGroupAddon>
              {query && (
                <InputGroupAddon align="inline-end">
                  <InputGroupButton aria-label="Clear title filter" onClick={() => handleQueryChange("")}>
                    <X className="h-4 w-4" />
                  </InputGroupButton>
                </InputGroupAddon>
              )}
            </InputGroup></div>

            <div className="flex flex-wrap items-center justify-start gap-3 text-sm text-muted-foreground xl:justify-end">
              {projectControl}
              <div hidden={tab !== "archive"}><Select value={sortMode} onValueChange={(value) => setSortMode(value as SortMode)}>
                <SelectTrigger className="h-9 w-36 bg-background">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent align="end">
                  <SelectItem value="newest">Newest first</SelectItem>
                  <SelectItem value="oldest">Oldest first</SelectItem>
                  <SelectItem value="title">Title A-Z</SelectItem>
                </SelectContent>
              </Select></div>
              <span>{meetings.length} total</span>
              <span className="h-1 w-1 rounded-full bg-muted-foreground/40" />
              <span>{shownCount} shown</span>
            </div>
          </div>
        </PageSection>

        <div className="min-h-0">
          <PageSection className="min-w-0 rounded-lg border border-border bg-card shadow-sm">
            <div className="flex items-center justify-between gap-4 border-b border-border px-6 py-5">
              <div>
                <h2 className="text-lg font-semibold text-foreground">
                  Saved meetings
                </h2>
                <p className="mt-1 text-sm text-muted-foreground">
                  {normalizedQuery
                    ? `Results for "${query.trim()}"`
                    : "Your meeting archive"}
                </p>
              </div>
              <NotebookPen className="h-5 w-5 text-primary" />
            </div>

            {visibleMeetings.length > 0 ? (
              <div className="divide-y divide-border">
                {visibleMeetings.map((meeting) => (
                  <Button variant="ghost"
                    key={meeting.id}
                    onClick={() => openMeeting(meeting.id, meeting.title)}
                    className="group grid h-auto w-full justify-stretch whitespace-normal rounded-none gap-4 px-6 py-5 text-left transition hover:bg-muted lg:grid-cols-[minmax(0,1fr)_auto] lg:items-center"
                  >
                    <div className="flex min-w-0 items-center gap-3">
                      <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-md border border-border bg-muted text-primary">
                        <FileText className="h-4 w-4" />
                      </span>
                      <div className="min-w-0">
                        <h3 className="truncate text-sm font-semibold text-foreground">
                          {meeting.title}
                        </h3>
                        <p className="mt-1 truncate text-xs text-primary">{projectTags.filter(tag => tag.meeting_id === meeting.id).map(tag => tag.tag).join(' · ')}</p>
                        <div className="mt-1 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                          <Clock3 className="h-3.5 w-3.5" />
                          <span>{formatMeetingDate(meeting.created_at)}</span>
                        </div>
                      </div>
                    </div>
                    <span className="inline-flex items-center gap-2 text-sm font-medium text-muted-foreground transition group-hover:text-primary lg:justify-self-end">
                      Open details
                      <ArrowRight className="h-4 w-4 transition group-hover:translate-x-0.5" />
                    </span>
                  </Button>
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
          </PageSection>

        </div>
          </TabsContent>
        </Tabs>
      </div>
    </div>
  );
}
