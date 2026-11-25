"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AlertCircle,
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
import { useRouter } from "next/navigation";
import { useSidebar } from "@/components/Sidebar/SidebarProvider";
import { useImportDialog } from "@/contexts/ImportDialogContext";
import { useRecordingState } from "@/contexts/RecordingStateContext";
import { useConfig } from "@/contexts/ConfigContext";
import { LibraryBackup } from "@/components/LibraryBackup";
import { ArchiveMeetingRows, useArchiveMetadata } from "@/components/Archive/ArchiveMeetingRows";
import { ArchiveStorageButton } from "@/components/Archive/ArchiveStorageButton";
import { ArchiveBulkActions, type BulkAction } from "@/components/Archive/ArchiveBulkActions";
import type { ArchiveMeeting } from "@/components/Archive/ArchiveAudioRemoval";
import { OpenMeetingActions } from "@/components/Archive/OpenMeetingActions";
import { archiveDateKey, archiveReview, parseArchiveDate } from "@/lib/archive-review";
import { receiveMatchScope, savedPassageUrl, sourceLabel, supportsPersonalExcerpts, type MatchScope, type SavedHit } from "@/lib/archive-search";
import { SearchSnippet } from "@/components/Archive/SearchSnippet";
import { KnowledgeArchive } from "@/components/Knowledge/KnowledgeArchive";
import { bookmarkTime, matchesProjectFilter, type ProjectFilter } from "@/lib/library";
import { libraryScope } from "@/lib/knowledge-state";
import { transcriptPassageUrl } from "@/lib/knowledge-navigation";
import { useResultNotes } from "@/hooks/useResultNotes";
import { useLibrarySearch, type TranscriptHit } from "@/hooks/useLibrarySearch";
import { useKnowledgeIndexStatus } from "@/hooks/useKnowledgeIndexStatus";
import { useDebouncedDiscovery } from "@/hooks/useDebouncedDiscovery";
import { mergeArchivePassages } from "@/lib/archive-hybrid-search";
import { knowledgeService } from "@/services/knowledgeService";
import type { KnowledgeScope, SearchMode } from "@/types/knowledge";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { ProjectFilterMenu } from "@/components/Knowledge/ProjectFilterMenu";
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
let archiveView = { tab: "archive" as "archive" | "memory" | "actions", memoryProject: noProjectFilter, query: "", from: "", to: "", attentionOnly: false, sortMode: "newest" as SortMode, projectFilter: noProjectFilter, searchMode: "keyword" as SearchMode };

export default function MeetingsPage() {
  const router = useRouter();
  const { openImportDialog } = useImportDialog();
  const recording = useRecordingState();
  const { modelConfig } = useConfig();
  const {
    meetings,
    meetingsError,
    meetingsLoading,
    setCurrentMeeting,
    handleRecordingToggle,
    refetchMeetings,
    projectTags,
  } = useSidebar();
  const [query, setQuery] = useState(archiveView.query);
  const [from, setFrom] = useState(archiveView.from);
  const [to, setTo] = useState(archiveView.to);
  const [tab, setTab] = useState(archiveView.tab);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [sortMode, setSortMode] = useState(archiveView.sortMode);
  const [projectFilter, setProjectFilter] = useState(archiveView.projectFilter);
  const [memoryProject, setMemoryProject] = useState(archiveView.memoryProject);
  const [attentionOnly, setAttentionOnly] = useState(archiveView.attentionOnly);
  const [searchMode, setSearchMode] = useState(archiveView.searchMode);
  const [bulk, setBulk] = useState<{ action: BulkAction; meetings: ArchiveMeeting[] } | null>(null);
  const bulkOpener = useRef<HTMLElement | null>(null);
  const archiveTab = useRef<HTMLButtonElement>(null);
  const [matchSession, setMatchSession] = useState<{ scope: MatchScope; key: number } | null>(null);
  const askAboutMatches = useCallback((scope: MatchScope) => {
    if (!scope.ids.length) return;
    setMemoryProject(scope.project);
    setMatchSession(last => ({ scope, key: (last?.key ?? 0) + 1 }));
    setTab("memory");
  }, []);
  useEffect(() => receiveMatchScope(askAboutMatches), [askAboutMatches]);
  useEffect(() => {
    const navigate = (target: unknown) => {
      if (target === "archive" || target === "memory" || target === "actions") setTab(target);
    };
    const onTab = (event: Event) => navigate((event as CustomEvent).detail);
    if (typeof window === "undefined") return;
    navigate(new URLSearchParams(window.location?.search).get("tab"));
    window.addEventListener("open-meetings-tab", onTab);
    return () => window.removeEventListener("open-meetings-tab", onTab);
  }, []);
  useEffect(() => { archiveView = { query, from, to, sortMode, attentionOnly, projectFilter, tab, memoryProject, searchMode }; }, [query, from, to, sortMode, attentionOnly, projectFilter, tab, memoryProject, searchMode]);
  const [archiveRevision, setArchiveRevision] = useState(0);
  const archiveStatus = useArchiveMetadata(tab === "archive", archiveRevision, meetings.length);
  const attentionIds = useMemo(() => new Set(Array.from(archiveStatus.metadata?.values() ?? [])
    .filter(row => archiveReview(row).needsAttention).map(row => row.id)), [archiveStatus.metadata]);

  const hasQuery = !!query.trim();
  const hasScope = !!(attentionOnly || from || to || projectFilter.untagged || projectFilter.tags.length);
  const invalidDates = !!from && !!to && from > to;
  const scope = hasScope ? libraryScope(attentionOnly ? [...attentionIds] : [], !attentionOnly, projectFilter, from, to) : undefined;
  const searchActive = tab === "archive" && !invalidDates && (!attentionOnly || attentionIds.size > 0);
  const { results, searching: literalSearching, error: searchError } = useLibrarySearch(query, searchActive, scope, false, meetings);
  const { semanticReady, index } = useKnowledgeIndexStatus(tab === "archive");
  const mode = semanticReady ? searchMode : "keyword";
  const scopeKey = JSON.stringify(scope ?? libraryScope([], true, noProjectFilter));
  const hybridSearch = useCallback((text: string) => knowledgeService.search({
    scope: JSON.parse(scopeKey) as KnowledgeScope, query: text, document_ids: [], mode: "hybrid",
  }), [scopeKey]);
  const hybrid = useDebouncedDiscovery(query, searchActive && mode === "hybrid" && !searchError && Array.from(query.trim()).length <= 256, hybridSearch, meetings);
  const searching = literalSearching || hybrid.loading;
  const noteSelection = useResultNotes(JSON.stringify({ query, projectFilter, attentionOnly, from, to, mode }), supportsPersonalExcerpts(modelConfig?.provider));
  const titleMatches = useMemo(() => new Map(results?.titles.map(hit => [hit.id, hit])), [results]);

  const sortedMeetings = useMemo(() => {
    const next = meetings.filter(meeting => {
      if (invalidDates || (attentionOnly && !attentionIds.has(meeting.id)) || !matchesProjectFilter(meeting.id, projectFilter, projectTags)) return false;
      if (!from && !to) return true;
      const date = archiveDateKey(meeting.created_at);
      return (!from || (!!date && date >= from)) && (!to || (!!date && date <= to));
    });
    next.sort((a, b) => {
      if (sortMode === "title") {
        return a.title.localeCompare(b.title, undefined, { sensitivity: "base" });
      }
      const aTime = parseArchiveDate(a.created_at)?.getTime() ?? 0;
      const bTime = parseArchiveDate(b.created_at)?.getTime() ?? 0;
      if (aTime !== bTime) {
        return sortMode === "oldest" ? aTime - bTime : bTime - aTime;
      }
      return a.title.localeCompare(b.title, undefined, { sensitivity: "base" });
    });
    return next;
  }, [meetings, sortMode, projectFilter, projectTags, from, to, invalidDates, attentionOnly, attentionIds]);
  const memoryMeetings = useMemo(() => meetings.filter(meeting => matchesProjectFilter(meeting.id, memoryProject, projectTags)), [meetings, memoryProject, projectTags]);
  const excerpts = useMemo(() => {
    const groups = new Map<string, { passages: TranscriptHit[]; saved: SavedHit[] }>();
    const group = (id: string) => {
      const value = groups.get(id) ?? { passages: [], saved: [] };
      groups.set(id, value);
      return value;
    };
    const passages = mergeArchivePassages(results?.passages ?? [], hybrid.data?.mode === "hybrid" ? hybrid.data.passages : [])
      .sort((a, b) => (a.start_seconds ?? Infinity) - (b.start_seconds ?? Infinity));
    for (const hit of passages) group(hit.meeting_id).passages.push(hit);
    for (const hit of results?.saved ?? []) group(hit.reference.meeting_id).saved.push(hit);
    return groups;
  }, [results, hybrid.data]);
  const visibleMeetings = hasQuery ? sortedMeetings.filter(meeting => titleMatches.has(meeting.id) || excerpts.has(meeting.id)) : sortedMeetings;
  const matchIds = hasQuery && results ? visibleMeetings.map(meeting => meeting.id) : [];

  const openPassage = (hit: TranscriptHit, play: boolean) => {
    const url = transcriptPassageUrl(hit.evidence, true, play);
    if (!url) return;
    setCurrentMeeting({ id: hit.meeting_id, title: hit.title });
    router.push(url);
  };

  const renderExcerpts = (meeting: { id: string }) => {
    const group = excerpts.get(meeting.id);
    if (!group) return null;
    return <ul className="mt-3 space-y-3 border-l border-border pl-3 sm:ml-2 sm:pl-4">
      {group.passages.map(hit => <li key={hit.evidence.chunk_id} className="space-y-1">
        <div className="flex flex-wrap items-center justify-between gap-1">
          <p className="text-xs text-muted-foreground">Transcript{hit.speaker ? ` · ${hit.speaker}` : ""}{hit.start_seconds != null ? ` · ${bookmarkTime(hit.start_seconds)}` : ""}</p>
          <div className="flex flex-wrap gap-1">
            <Button variant="ghost" size="sm" onClick={() => openPassage(hit, false)}>Show in transcript</Button>
            {hit.start_seconds != null && Number.isFinite(hit.start_seconds) && hit.start_seconds >= 0 && <Button variant="outline" size="sm" onClick={() => openPassage(hit, true)}>Play passage</Button>}
          </div>
        </div>
        <blockquote className="whitespace-pre-wrap break-words text-sm text-muted-foreground"><SearchSnippet {...hit} /></blockquote>
      </li>)}
      {group.saved.map(hit => <li key={hit.reference.fingerprint} className="space-y-1">
        <div className="flex flex-wrap items-center justify-between gap-1">
          <p className="text-xs text-muted-foreground">{sourceLabel[hit.reference.source]}{hit.seconds != null && ` · ${bookmarkTime(hit.seconds)}`}</p>
          <Button variant="ghost" size="sm" onClick={() => {
            setCurrentMeeting({ id: hit.reference.meeting_id, title: hit.title });
            router.push(savedPassageUrl(hit.reference, true));
          }}>Show {sourceLabel[hit.reference.source].toLowerCase()}</Button>
        </div>
        <blockquote className="whitespace-pre-wrap break-words text-sm text-muted-foreground"><SearchSnippet {...hit} /></blockquote>
        {noteSelection.control(hit)}
      </li>)}
    </ul>;
  };

  const handleRefresh = useCallback(async () => {
    setIsRefreshing(true);
    try {
      await refetchMeetings();
    } finally {
      setArchiveRevision(current => current + 1);
      setIsRefreshing(false);
    }
  }, [refetchMeetings]);

  const shownCount = visibleMeetings.length;
  const clearFilters = () => { setQuery(""); setProjectFilter(noProjectFilter); setFrom(""); setTo(""); setAttentionOnly(false); };
  const recordingBusy = recording.isRecording || recording.isStarting || recording.isStopping || recording.isProcessing || recording.isSaving;

  return (
    <div className="h-full min-h-0 overflow-hidden bg-background text-foreground">
      <div className="mx-auto flex h-full min-h-0 w-full max-w-[1600px] flex-col gap-4 px-5 py-5">
        <header className="flex shrink-0 flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
          <div>
            <h1 className="text-3xl font-semibold tracking-tight text-foreground">
              Meetings
            </h1>
          </div>

          {tab === "archive" && <div className="flex flex-wrap items-center gap-2"><LibraryBackup /><ArchiveStorageButton /><Button variant="outline" disabled={isRefreshing}
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

        <Tabs value={tab} onValueChange={value => setTab(value as typeof tab)} className="flex min-h-0 flex-1 flex-col gap-4">
          <TabsList aria-label="Meetings views" className="w-fit shrink-0">
            <TabsTrigger ref={archiveTab} value="archive" id="meetings-archive-tab" aria-controls="meetings-archive-panel">Archive</TabsTrigger>
            <TabsTrigger value="memory" id="meetings-memory-tab" aria-controls="meeting-memory-panel">Meeting memory</TabsTrigger>
            <TabsTrigger value="actions" id="meetings-actions-tab" aria-controls="meetings-actions-panel">Open actions</TabsTrigger>
          </TabsList>
          <TabsContent value="memory" forceMount id="meeting-memory-panel" hidden={tab !== "memory"} className={tab === "memory" ? "m-0 min-h-0 flex-1" : "hidden"}>
            <KnowledgeArchive key={matchSession?.key ?? 0} initialMatches={matchSession?.scope} active={tab === "memory"} meetings={memoryMeetings} meetingTitles={meetings} projectFilter={memoryProject} setProjectFilter={setMemoryProject} projectControl={<ProjectFilterMenu filter={memoryProject} setFilter={setMemoryProject} />} />
          </TabsContent>
          <TabsContent value="actions" id="meetings-actions-panel" className="m-0 min-h-0 flex-1 overflow-y-auto">
            <OpenMeetingActions />
          </TabsContent>
          <TabsContent value="archive" forceMount id="meetings-archive-panel" hidden={tab !== "archive"} className={tab === "archive" ? "m-0 min-h-0 flex-1 space-y-4 overflow-y-auto" : "hidden"}>
        {meetings.length > 0 && (
          <div className="grid gap-4 xl:grid-cols-[1fr_auto] xl:items-center">
            <div hidden={tab !== "archive"}><InputGroup className="rounded-md border-border bg-background text-foreground shadow-none">
              <InputGroupInput
                id="meetings-search"
                aria-label="Search titles, transcripts, summaries, notes and bookmarks"
                placeholder='Search meetings. Use quotes for phrases.'
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
              <Select value={mode} onValueChange={value => setSearchMode(value as SearchMode)}>
                <SelectTrigger aria-label="Archive search mode" className="h-9 w-32"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="keyword">Keyword</SelectItem>
                  <SelectItem value="hybrid" disabled={!semanticReady}>{semanticReady ? "Hybrid" : index?.semantic_enabled ? "Hybrid (preparing)" : "Hybrid (enable in Settings)"}</SelectItem>
                </SelectContent>
              </Select>
              <ProjectFilterMenu filter={projectFilter} setFilter={setProjectFilter} />
              {(attentionOnly || attentionIds.size > 0) && <Button variant={attentionOnly ? "secondary" : "ghost"} size="sm" aria-pressed={attentionOnly} onClick={() => setAttentionOnly(value => !value)}>Needs attention</Button>}
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
              {!hasQuery && !invalidDates && <span className="whitespace-nowrap" aria-live="polite">{hasScope ? `${shownCount} of ${meetings.length} meeting${meetings.length !== 1 ? 's' : ''}` : `${meetings.length} meeting${meetings.length !== 1 ? 's' : ''}`}</span>}
            </div>
          </div>
        )}

        <div className="min-h-0">
          {attentionOnly && archiveStatus.error && <p role="alert" className="text-sm text-destructive">Could not refresh meetings that need attention. <Button variant="ghost" size="sm" onClick={() => void archiveStatus.refresh()}>Retry</Button></p>}
          {invalidDates && <p role="alert" className="text-sm text-destructive">The start date must be on or before the end date.</p>}
          {searching && <p role="status" className="text-sm text-muted-foreground">Searching saved meetings…</p>}
          {searchError && <p role="alert" className="text-sm text-destructive">{searchError}</p>}
          {(hybrid.error || hybrid.data?.mode === "keyword") && <p role="status" className="text-sm text-muted-foreground">Hybrid search is unavailable for this query. Showing keyword matches.</p>}
          {results?.limited && <p role="status" className="mb-3 text-sm text-muted-foreground">Search reached its limit. Try more specific text or narrow the filters.</p>}
          {meetingsError && meetings.length > 0 && (
            <div role="alert" className="mb-3 flex flex-wrap items-center justify-between gap-3 rounded-md border border-error-border/30 bg-error px-4 py-3 text-sm text-error-foreground">
              <span>Could not refresh your meetings. The list may be out of date.</span>
              <Button variant="outline" size="sm" disabled={meetingsLoading} onClick={() => void refetchMeetings()}>{meetingsLoading ? "Retrying…" : "Retry"}</Button>
            </div>
          )}
          {matchIds.length > 0 && <Button variant="outline" size="sm" className="mb-3" onClick={() => askAboutMatches({ ids: matchIds, project: projectFilter, from, to, question: query.trim(), notes: noteSelection.references(matchIds.flatMap(id => excerpts.get(id)?.saved ?? [])) })}>Ask about these results</Button>}
          {!invalidDates && !searching && !searchError && (!hasQuery || visibleMeetings.length > 0 || !results?.limited) && <PageSection className="min-w-0 rounded-lg border border-border bg-card shadow-sm" aria-label={hasQuery ? "Matching meetings" : "Saved meetings"}>
            {hasQuery && visibleMeetings.length > 0 && <h2 className="border-b border-border px-6 py-3 text-sm font-semibold">Matching meetings</h2>}
            {visibleMeetings.length > 0 ? (
              <ArchiveMeetingRows
                status={archiveStatus}
                meetings={visibleMeetings}
                projectTags={projectTags}
                onOpen={setCurrentMeeting}
                onBulk={(action, meetings, opener) => { bulkOpener.current = opener; setBulk({ action, meetings }); }}
                onAsk={ids => askAboutMatches({ ids, project: noProjectFilter, from: "", to: "" })}
                renderTitle={hasQuery ? meeting => <SearchSnippet text={meeting.title} highlights={titleMatches.get(meeting.id)?.title === meeting.title ? titleMatches.get(meeting.id)?.highlights : undefined} /> : undefined}
                renderExcerpts={hasQuery ? renderExcerpts : undefined}
              />
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

        </div>
          </TabsContent>
        </Tabs>
        {bulk && <ArchiveBulkActions {...bulk} onClose={() => setBulk(null)}
          onDeleted={() => void archiveStatus.refresh()}
          onReturnFocus={() => { if (bulkOpener.current?.isConnected) bulkOpener.current.focus(); else archiveTab.current?.focus(); }} />}
      </div>
    </div>
  );
}
