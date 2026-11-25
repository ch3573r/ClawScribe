"use client";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { ChevronDown, X } from "lucide-react";
import { useSummaryConfig } from "@/contexts/ConfigContext";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { knowledgeSearchMode, libraryScope, scopeReady } from "@/lib/knowledge-state";
import { useKnowledgeIndexStatus } from "@/hooks/useKnowledgeIndexStatus";
import { useKnowledgeSearch } from "@/hooks/useKnowledgeSearch";
import { SearchResults } from "./SearchResults";
import { KnowledgeChat } from "./KnowledgeChat";
import { EvidencePreview } from "./EvidencePreview";
import { ReferenceDocuments } from "@/components/MeetingDetails/ReferenceDocuments";
import { AttachReferenceButton } from "@/components/MeetingDetails/AttachReferenceButton";
import type { ProjectFilter } from "@/lib/library";
import { supportsPersonalExcerpts, type MatchScope, type SavedReference } from "@/lib/archive-search";
import { classifySummaryProvider } from "@/lib/privacy";
import { archiveDateKey } from "@/lib/archive-review";
import type { DocumentAttachment, MeetingFilter, SearchMode } from "@/types/knowledge";

// Route navigation unmounts the archive; keep only the user's explicit source
// choices in this window. Document selection and sharing remain owner-scoped.
let memorySources = { all: false, selected: [] as string[], from: "", to: "" };

export function KnowledgeArchive({ active = true, initialMatches, meetings, meetingTitles = meetings, projectFilter, setProjectFilter, projectControl }: {
  initialMatches?: MatchScope;
  active?: boolean; meetings: { id: string; title: string; created_at?: string }[]; meetingTitles?: { id: string; title: string }[]; projectFilter: ProjectFilter; setProjectFilter?: (filter: ProjectFilter) => void; projectControl?: ReactNode;
}) {
  const { modelConfig } = useSummaryConfig();
  const destination = classifySummaryProvider(modelConfig?.provider, {
    ollamaEndpoint: modelConfig?.ollamaEndpoint, customEndpoint: modelConfig?.customOpenAIEndpoint,
  });
  const [query, setQuery] = useState("");
  const [chosenMode, setChosenMode] = useState<SearchMode | null>(null);
  const [all, setAll] = useState(initialMatches ? false : memorySources.all);
  const [selected, setSelected] = useState(initialMatches?.ids ?? memorySources.selected);
  const [notes, setNotes] = useState<SavedReference[]>(initialMatches?.notes ?? []);
  const excerptsSupported = supportsPersonalExcerpts(modelConfig?.provider);
  useEffect(() => { if (!excerptsSupported) setNotes([]); }, [excerptsSupported]);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [meetingQuery, setMeetingQuery] = useState("");
  const [from, setFrom] = useState(initialMatches?.from ?? memorySources.from);
  const [to, setTo] = useState(initialMatches?.to ?? memorySources.to);
  useEffect(() => { memorySources = { all, selected, from, to }; }, [all, selected, from, to]);
  const [searchOpen, setSearchOpen] = useState(false);
  const [toolsOpen, setToolsOpen] = useState(false);
  const [referenceReveal, setReferenceReveal] = useState(0);
  const { index, indexError, semanticReady } = useKnowledgeIndexStatus(active);
  const mode = knowledgeSearchMode(chosenMode, semanticReady);
  const scope = useMemo(() => libraryScope(selected, all, projectFilter, from, to), [selected, all, projectFilter, from, to]);
  const state = useKnowledgeSearch(scope, null, { provider: modelConfig?.provider, model: modelConfig?.model }, {
    active,
    draftMeetingIds: selected,
    restoreScope: (filter: MeetingFilter | null) => {
      setAll(filter?.all_meetings ?? false);
      setSelected(filter?.meeting_ids ?? []);
      setFrom(filter?.from ?? "");
      setTo(filter?.to ?? "");
      setProjectFilter?.({ tags: filter?.tags ?? [], untagged: filter?.untagged ?? false, mode: filter?.tag_mode ?? "any" });
    },
  });
  const matchingMeetings = meetings.filter(meeting => meeting.title.toLocaleLowerCase().includes(meetingQuery.trim().toLocaleLowerCase()));
  const invalidDates = !!from && !!to && from > to;
  const sourcesReady = scopeReady(scope);
  const attachmentMeetings = meetings.filter(meeting => {
    const date = archiveDateKey(meeting.created_at);
    return !invalidDates && (!from || (!!date && date >= from)) && (!to || (!!date && date <= to));
  });
  const attachmentMeetingId = !all && selected.length === 1 && attachmentMeetings.some(meeting => meeting.id === selected[0]) ? selected[0] : undefined;
  const [attachmentBusy, setAttachmentBusy] = useState(false);
  const attached = (id: string, attachment: DocumentAttachment) => {
    if (!all && (selected.length > 0 || !sourcesReady)) setSelected(ids => [...new Set([...ids, id])]);
    state.selectImportedDocument(attachment); setReferenceReveal(value => value + 1); setToolsOpen(true);
  };
  const secondary = <div className="space-y-3">
    <Button variant="ghost" size="sm" className="text-muted-foreground" aria-expanded={toolsOpen} aria-controls="memory-source-tools" onClick={() => setToolsOpen(value => !value)}>
      <ChevronDown className={toolsOpen ? "rotate-180" : ""} />Reference documents and source search
    </Button>
    <div id="memory-source-tools" hidden={!toolsOpen} className="space-y-3">
    <ReferenceDocuments state={state} provider={modelConfig?.provider} reveal={referenceReveal} />
    <Button variant="ghost" size="sm" className="text-muted-foreground" aria-expanded={searchOpen} aria-controls="transcript-search" onClick={() => setSearchOpen(value => !value)}>
      <ChevronDown className={searchOpen ? "rotate-180" : ""} />Source search
    </Button>
    {searchOpen && <div id="transcript-search" className="space-y-3 border-b border-border pb-4">
      <form className="flex flex-wrap items-end gap-3" onSubmit={event => { event.preventDefault(); if (sourcesReady && query.trim() && !state.loading) void state.search(query, mode); }}>
        <label className="min-w-40 flex-1 space-y-1 text-xs font-medium">Search sources
          <Input value={query} onChange={event => { setQuery(event.target.value); state.cancel(); }} placeholder="Search what was said…" />
        </label>
        <div className="w-52 space-y-1"><label htmlFor="memory-search-mode" className="text-xs font-medium">Search mode</label>
          <Select value={mode} onValueChange={value => { setChosenMode(value as SearchMode); state.cancel(); }}>
            <SelectTrigger id="memory-search-mode"><SelectValue /></SelectTrigger>
            <SelectContent><SelectItem value="keyword">Keyword</SelectItem>
              <SelectItem value="hybrid" disabled={!semanticReady}>{semanticReady ? "Semantic + keyword" : index?.semantic_enabled ? "Semantic — preparing" : "Semantic — enable in Settings"}</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <Button variant="outline" disabled={!sourcesReady || state.loading || !query.trim()} type="submit">{state.loading ? "Searching…" : "Search sources"}</Button>
      </form>
      {indexError && <p role="status" className="text-xs text-muted-foreground">Search status is unavailable. Keyword search is available.</p>}
      <SearchResults state={state} />
    </div>}
    </div>
  </div>;
  const sources = <fieldset className="min-w-0 space-y-3">
      <legend className="mb-2 text-sm font-medium">Sources</legend>
      {sourcesReady && <p className="text-sm text-muted-foreground">
        Asking uses {all ? "all meetings in this filter" : !selected.length ? "meetings matching these filters" : `${selected.length} selected meeting${selected.length === 1 ? "" : "s"}`}.
        {" "}{notes.length ? `${notes.length} personal excerpt${notes.length === 1 ? "" : "s"} selected for review before sending.` : "Personal notes and bookmarks stay on this device."}
      </p>}
      <div className="flex flex-wrap items-center gap-3 text-sm [&_button]:text-sm">
        <label className="flex items-center gap-2 text-sm"><Switch checked={all} onCheckedChange={setAll} />Search all saved meetings</label>
        {!all && <Popover open={pickerOpen} onOpenChange={setPickerOpen}><PopoverTrigger asChild><Button variant="outline" size="sm">Choose meetings{selected.length ? ` (${selected.length})` : ""}</Button></PopoverTrigger>
          <PopoverContent align="start" className="w-96 max-w-[calc(100vw-3rem)] space-y-3">
            <Input aria-label="Find meetings" value={meetingQuery} onChange={event => setMeetingQuery(event.target.value)} placeholder="Find a meeting…" />
            <ScrollArea className="h-56"><div className="space-y-3 pr-3">
              {matchingMeetings.map(meeting => <label key={meeting.id} className="flex items-start gap-2 text-sm">
                <Checkbox checked={selected.includes(meeting.id)} className="mt-0.5" onCheckedChange={checked => setSelected(ids => checked === true ? [...new Set([...ids, meeting.id])] : ids.filter(id => id !== meeting.id))} />
                <span className="break-words">{meeting.title}</span>
              </label>)}
              {!matchingMeetings.length && <p className="text-xs text-muted-foreground">No matching meetings.</p>}
            </div></ScrollArea>
          </PopoverContent>
        </Popover>}
        {projectControl}
        <Popover><PopoverTrigger asChild><Button variant="outline" size="sm">Date range{from || to ? " · active" : ""}</Button></PopoverTrigger>
          <PopoverContent align="start" className="space-y-3"><div className="grid grid-cols-2 gap-3">
            <label className="space-y-1 text-xs">From<Input type="date" value={from} onChange={event => setFrom(event.target.value)} /></label>
            <label className="space-y-1 text-xs">To<Input type="date" value={to} onChange={event => setTo(event.target.value)} /></label>
          </div><p className="text-xs text-muted-foreground">Includes both dates.</p>
            {(from || to) && <Button variant="ghost" size="sm" onClick={() => { setFrom(""); setTo(""); }}>Clear dates</Button>}
          </PopoverContent>
        </Popover>
        <AttachReferenceButton meetingId={attachmentMeetingId} meetings={attachmentMeetings} scopeKey={JSON.stringify([scope, state.owner])} onBusyChange={setAttachmentBusy} onAttached={attached} />
      </div>
      {!all && selected.length > 0 && <TooltipProvider><div className="flex flex-wrap gap-2" aria-label="Selected meetings">
        {selected.slice(0, 3).map(id => {
          const title = meetingTitles.find(meeting => meeting.id === id)?.title ?? "Unavailable meeting";
          return <Tooltip key={id}><TooltipTrigger asChild><Button type="button" variant="outline" size="sm" className="max-w-52 gap-1 bg-primary/10" aria-label={`Remove ${title}`} onClick={() => setSelected(ids => ids.filter(value => value !== id))}>
            <span className="truncate">{title}</span><X className="h-3 w-3 shrink-0" />
          </Button></TooltipTrigger><TooltipContent>{title}</TooltipContent></Tooltip>;
        })}
        {selected.length > 3 && <Button variant="ghost" size="sm" onClick={() => setPickerOpen(true)}>+{selected.length - 3} meetings</Button>}
      </div></TooltipProvider>}
      {invalidDates && <p role="alert" className="text-sm text-destructive">The start date must be on or before the end date.</p>}
    </fieldset>;
  return <section className="h-full min-h-0 [container-name:meeting-memory] [container-type:size]" aria-label="Meeting memory">
    <KnowledgeChat state={state} mode={mode} provider={modelConfig?.provider} model={modelConfig?.model}
      secondary={<div className="space-y-4">{sources}{secondary}</div>} attachmentBusy={attachmentBusy}
      initialQuestion={initialMatches?.question} selectedNotes={excerptsSupported ? notes : undefined} reviewActive={active} destinationLabel={destination.label} destinationRevision={JSON.stringify([modelConfig?.ollamaEndpoint, modelConfig?.customOpenAIEndpoint, modelConfig?.organization, modelConfig?.project])}
      removeNote={ref => setNotes(current => current.filter(value => value.fingerprint !== ref.fingerprint))}
      clearNotes={() => setNotes([])}
      emptyState={!state.owner && !sourcesReady ? <div className="flex flex-col items-center gap-2 text-center">
        <h3 className="text-lg font-semibold">Choose meetings to ask about</h3>
        <p className="text-sm text-muted-foreground">Pick meetings, filter by project or date, or search all saved meetings.</p>
        <Button className="mt-4" onClick={() => setPickerOpen(true)}>Choose meetings</Button>
      </div> : undefined}
    />
    {active && <EvidencePreview state={state} />}
  </section>;
}
