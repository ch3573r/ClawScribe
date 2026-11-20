"use client";

import { useCallback, useEffect, useMemo, useState, type Dispatch, type SetStateAction } from "react";
import {
  AlertCircle,
  ChevronDown,
  ChevronRight,
  FileText,
  Mic,
  RefreshCw,
  Search,
  Upload,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { PageSection } from "@/components/ui/page-section";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useSidebar } from "@/components/Sidebar/SidebarProvider";
import { useImportDialog } from "@/contexts/ImportDialogContext";
import { useRecordingState } from "@/contexts/RecordingStateContext";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { LibraryBackup } from "@/components/LibraryBackup";
import { KnowledgeArchive } from "@/components/Knowledge/KnowledgeArchive";
import { bookmarkTime, matchesProjectFilter, type ProjectFilter } from "@/lib/library";
import { libraryScope } from "@/lib/knowledge-state";
import { transcriptPassageUrl } from "@/lib/knowledge-navigation";
import { useLibrarySearch } from "@/hooks/useLibrarySearch";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
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
const noProjectFilter: ProjectFilter = { tags: [], untagged: false, mode: "any" };
// Keep this view's filters in memory when a meeting route replaces it.
let archiveView = { query: "", from: "", to: "", sortMode: "newest" as SortMode, projectFilter: noProjectFilter };

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
  const { openImportDialog } = useImportDialog();
  const recording = useRecordingState();
  const {
    meetings,
    meetingsError,
    meetingsLoading,
    setCurrentMeeting,
    handleRecordingToggle,
    refetchMeetings,
    projectTags,
    projectTagsError,
    projectTagsLoading,
    refreshProjectTags,
  } = useSidebar();
  const [query, setQuery] = useState(archiveView.query);
  const [from, setFrom] = useState(archiveView.from);
  const [to, setTo] = useState(archiveView.to);
  const [tab, setTab] = useState<"archive" | "memory">("archive");
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [sortMode, setSortMode] = useState(archiveView.sortMode);
  const [projectFilter, setProjectFilter] = useState(archiveView.projectFilter);
  const [memoryProject, setMemoryProject] = useState(noProjectFilter);
  useEffect(() => { archiveView = { query, from, to, sortMode, projectFilter }; }, [query, from, to, sortMode, projectFilter]);
  const tagOptions = useMemo(() => [...new Map(projectTags.map(row => [row.tag.toLowerCase(), row.tag])).values()].sort((a, b) => a.localeCompare(b)), [projectTags]);

  const hasQuery = !!query.trim();
  const hasScope = !!(from || to || projectFilter.untagged || projectFilter.tags.length);
  const invalidDates = !!from && !!to && from > to;
  const scope = hasScope ? libraryScope([], true, projectFilter, from, to) : undefined;
  const { results, searching, error: searchError } = useLibrarySearch(query, tab === "archive" && !invalidDates, scope, false, meetings);
  const titleIds = useMemo(() => new Set(results?.titles.map(hit => hit.id)), [results]);

  const sortedMeetings = useMemo(() => {
    const next = meetings.filter(meeting => {
      if (invalidDates || !matchesProjectFilter(meeting.id, projectFilter, projectTags)) return false;
      const date = meeting.created_at?.slice(0, 10);
      return (!from || (!!date && date >= from)) && (!to || (!!date && date <= to));
    });
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
  }, [meetings, sortMode, projectFilter, projectTags, from, to, invalidDates]);
  const memoryMeetings = useMemo(() => meetings.filter(meeting => matchesProjectFilter(meeting.id, memoryProject, projectTags)), [meetings, memoryProject, projectTags]);
  const visibleMeetings = hasQuery ? sortedMeetings.filter(meeting => titleIds.has(meeting.id)) : sortedMeetings;
  const passages = useMemo(() => {
    const positions = new Map(sortedMeetings.map((meeting, index) => [meeting.id, index]));
    return (results?.passages ?? []).filter(hit => positions.has(hit.meeting_id)).sort((a, b) =>
      (positions.get(a.meeting_id)! - positions.get(b.meeting_id)!) ||
      (a.start_seconds ?? -1) - (b.start_seconds ?? -1));
  }, [results, sortedMeetings]);

  const handleRefresh = useCallback(async () => {
    setIsRefreshing(true);
    try {
      await refetchMeetings();
    } finally {
      setIsRefreshing(false);
    }
  }, [refetchMeetings]);

  const shownCount = visibleMeetings.length;
  const clearFilters = () => { setQuery(""); setProjectFilter(noProjectFilter); setFrom(""); setTo(""); };
  const recordingBusy = recording.isRecording || recording.isStarting || recording.isStopping || recording.isProcessing || recording.isSaving;

  const projectControl = (projectFilter: ProjectFilter, setProjectFilter: Dispatch<SetStateAction<ProjectFilter>>) => {
    const projectFilterLabel = projectFilter.untagged ? 'Untagged'
      : projectFilter.tags.length === 0 ? 'All projects'
      : projectFilter.tags.length === 1 ? projectFilter.tags[0]
      : `${projectFilter.tags.length} projects`;
    return <>
<div className="flex items-center gap-2 text-sm">Project
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button variant="ghost"
                      type="button"
                      aria-label={`Project: ${projectFilterLabel}`}
                      className="flex h-9 max-w-48 items-center justify-between gap-2 rounded-md border border-input bg-background px-3 py-2 text-sm shadow-sm disabled:cursor-not-allowed disabled:opacity-50"
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
  };

  return (
    <div className="h-full min-h-0 overflow-hidden bg-background text-foreground">
      <div className="mx-auto flex h-full min-h-0 w-full max-w-[1600px] flex-col gap-4 px-5 py-5">
        <header className="flex shrink-0 flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
          <div>
            <h1 className="text-3xl font-semibold tracking-tight text-foreground">
              Meetings
            </h1>
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
          <TabsContent value="memory" forceMount id="meeting-memory-panel" hidden={tab !== "memory"} className={tab === "memory" ? "m-0 min-h-0 flex-1" : "hidden"}>
            <KnowledgeArchive active={tab === "memory"} meetings={memoryMeetings} meetingTitles={meetings} projectFilter={memoryProject} setProjectFilter={setMemoryProject} projectControl={projectControl(memoryProject, setMemoryProject)} />
          </TabsContent>
          <TabsContent value="archive" forceMount id="meetings-archive-panel" hidden={tab !== "archive"} className={tab === "archive" ? "m-0 min-h-0 flex-1 space-y-4 overflow-y-auto" : "hidden"}>
        {meetings.length > 0 && (
          <div className="grid gap-4 xl:grid-cols-[1fr_auto] xl:items-center">
            <div hidden={tab !== "archive"}><InputGroup className="rounded-md border-border bg-background text-foreground shadow-none">
              <InputGroupInput
                id="meetings-search"
                aria-label="Search titles and transcripts"
                placeholder="Search titles and transcripts"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                className="placeholder:text-muted-foreground"
              />
              <InputGroupAddon>
                <Search className="h-4 w-4 text-muted-foreground" />
              </InputGroupAddon>
              {query && (
                <InputGroupAddon align="inline-end">
                  <InputGroupButton aria-label="Clear search" onClick={() => setQuery("")}>
                    <X className="h-4 w-4" />
                  </InputGroupButton>
                </InputGroupAddon>
              )}
            </InputGroup></div>

            <div className="flex flex-wrap items-center justify-start gap-3 text-sm text-muted-foreground xl:justify-end">
              {projectControl(projectFilter, setProjectFilter)}
              <Popover>
                <PopoverTrigger asChild><Button variant="outline" size="sm">Date range{from || to ? " · active" : ""}</Button></PopoverTrigger>
                <PopoverContent align="end" className="space-y-3">
                  <div className="grid grid-cols-2 gap-3">
                    <label className="space-y-1 text-xs">From<Input type="date" value={from} onChange={event => setFrom(event.target.value)} /></label>
                    <label className="space-y-1 text-xs">To<Input type="date" value={to} onChange={event => setTo(event.target.value)} /></label>
                  </div>
                  {(from || to) && <Button variant="ghost" size="sm" onClick={() => { setFrom(""); setTo(""); }}>Clear dates</Button>}
                </PopoverContent>
              </Popover>
              <div hidden={tab !== "archive"}><Select value={sortMode} onValueChange={(value) => setSortMode(value as SortMode)}>
                <SelectTrigger aria-label="Sort order" className="h-9 w-36 bg-background">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent align="end">
                  <SelectItem value="newest">Newest first</SelectItem>
                  <SelectItem value="oldest">Oldest first</SelectItem>
                  <SelectItem value="title">Title A-Z</SelectItem>
                </SelectContent>
              </Select></div>
              {!hasQuery && !invalidDates && <span className="whitespace-nowrap" aria-live="polite">{hasScope ? `${shownCount} of ${meetings.length} meetings` : `${meetings.length} meetings`}</span>}
            </div>
          </div>
        )}

        <div className="min-h-0">
          {invalidDates && <p role="alert" className="text-sm text-destructive">The start date must be on or before the end date.</p>}
          {searching && <p role="status" className="text-sm text-muted-foreground">Searching saved titles and transcripts…</p>}
          {searchError && <p role="alert" className="text-sm text-destructive">{searchError}</p>}
          {results?.limited && <p role="status" className="mb-3 text-sm text-muted-foreground">Search reached its limit. Try more specific text or narrow the filters.</p>}
          {meetingsError && meetings.length > 0 && (
            <div role="alert" className="mb-3 flex flex-wrap items-center justify-between gap-3 rounded-md border border-error-border/30 bg-error px-4 py-3 text-sm text-error-foreground">
              <span>Could not refresh your meetings. The list may be out of date.</span>
              <Button variant="outline" size="sm" disabled={meetingsLoading} onClick={() => void refetchMeetings()}>{meetingsLoading ? "Retrying…" : "Retry"}</Button>
            </div>
          )}
          {!invalidDates && !searching && !searchError && (!hasQuery || visibleMeetings.length > 0 || (!passages.length && !results?.limited)) && <PageSection className="min-w-0 rounded-lg border border-border bg-card shadow-sm" aria-label={hasQuery ? "Matching titles" : "Saved meetings"}>
            {hasQuery && visibleMeetings.length > 0 && <h2 className="border-b border-border px-6 py-3 text-sm font-semibold">Matching titles</h2>}
            {visibleMeetings.length > 0 ? (
              <TooltipProvider><div className="divide-y divide-border">
                {visibleMeetings.map((meeting) => (
                  <Tooltip key={meeting.id}>
                    <TooltipTrigger asChild><Link
                      href={`/meeting-details?id=${encodeURIComponent(meeting.id)}&from=archive`}
                      onClick={() => setCurrentMeeting({ id: meeting.id, title: meeting.title })}
                      className="flex w-full items-center gap-4 rounded-md px-6 py-4 text-left transition-colors hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
                    >
                      <div className="min-w-0 flex-1">
                        <h3 className="truncate text-[15px] font-medium text-foreground">
                          {meeting.title}
                        </h3>
                        <div className="mt-1 flex flex-wrap items-center gap-x-4 gap-y-1 text-sm text-muted-foreground">
                          <span>{formatMeetingDate(meeting.created_at)}</span>
                          <span>{projectTags.filter(tag => tag.meeting_id === meeting.id).map(tag => tag.tag).join(' · ')}</span>
                        </div>
                      </div>
                      <ChevronRight aria-hidden="true" className="h-4 w-4 shrink-0 text-muted-foreground" />
                    </Link></TooltipTrigger>
                    <TooltipContent className="max-w-sm break-words">{meeting.title}</TooltipContent>
                  </Tooltip>
                ))}
              </div></TooltipProvider>
            ) : meetingsError && meetings.length === 0 ? (
              <div role="alert" className="flex flex-col items-center justify-center px-6 py-16 text-center">
                <span className="flex h-12 w-12 items-center justify-center rounded-md border border-error-border/30 bg-error text-error-foreground">
                  <AlertCircle className="h-5 w-5" />
                </span>
                <h3 className="mt-4 text-base font-semibold text-foreground">Could not load your meetings</h3>
                <p className="mt-2 max-w-sm text-sm leading-6 text-muted-foreground">Your meetings are not lost. Check that ClawScribe is running normally, then try again.</p>
                <Button className="mt-6" disabled={meetingsLoading} onClick={() => void refetchMeetings()}>{meetingsLoading ? "Retrying…" : "Retry"}</Button>
              </div>
            ) : (
              <div className="flex flex-col items-center justify-center px-6 py-16 text-center">
                <span className="flex h-12 w-12 items-center justify-center rounded-md border border-border bg-muted text-primary">
                  <FileText className="h-5 w-5" />
                </span>
                <h3 className="mt-4 text-base font-semibold text-foreground">
                  {meetings.length === 0 ? "No meetings yet" : "No meetings match"}
                </h3>
                <p className="mt-2 max-w-sm text-sm leading-6 text-muted-foreground">
                  {meetings.length === 0 ? "Record a meeting or import a recording and it will appear here" : "Try different text or clear your filters."}
                </p>
                {meetings.length === 0 ? <div className="mt-6 flex flex-wrap justify-center gap-3">
                  <Button onClick={handleRecordingToggle} disabled={recordingBusy}><Mic className="h-4 w-4" />Start recording</Button>
                  <Button variant="outline" onClick={() => openImportDialog()}><Upload className="h-4 w-4" />Import audio or video</Button>
                </div> : <Button variant="outline" className="mt-6" onClick={clearFilters}>Clear filters</Button>}
              </div>
            )}
          </PageSection>}

          {passages.length > 0 && <PageSection aria-label="Matching transcript passages" className="mt-4 min-w-0">
            <h2 className="border-b border-border px-6 py-3 text-sm font-semibold">Matching transcript passages</h2>
            <ol className="divide-y divide-border">
              {passages.map(hit => <li key={hit.evidence.chunk_id} className="space-y-2 px-6 py-4">
                <div className="flex flex-wrap items-start justify-between gap-2">
                  <div className="min-w-0">
                    <h3 className="break-words text-sm font-medium">{hit.title}</h3>
                    <p className="text-xs text-muted-foreground">{formatMeetingDate(hit.date)}{hit.speaker ? ` · ${hit.speaker}` : ""}{hit.start_seconds != null ? ` · ${bookmarkTime(hit.start_seconds)}` : ""}</p>
                  </div>
                  <Button variant="outline" size="sm" aria-label={`Show passage in ${hit.title}`} onClick={() => {
                    const url = transcriptPassageUrl(hit.evidence, true);
                    if (!url) return;
                    setCurrentMeeting({ id: hit.meeting_id, title: hit.title });
                    router.push(url);
                  }}>Show in transcript</Button>
                </div>
                <blockquote className="whitespace-pre-wrap break-words text-sm text-muted-foreground">{hit.text}</blockquote>
              </li>)}
            </ol>
          </PageSection>}

        </div>
          </TabsContent>
        </Tabs>
      </div>
    </div>
  );
}
