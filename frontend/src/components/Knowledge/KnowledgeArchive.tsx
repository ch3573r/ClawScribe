"use client";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { ChevronDown, X } from "lucide-react";
import { useConfig } from "@/contexts/ConfigContext";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { libraryScope } from "@/lib/knowledge-state";
import { knowledgeService } from "@/services/knowledgeService";
import { useKnowledgeSearch } from "@/hooks/useKnowledgeSearch";
import { SearchResults } from "./SearchResults";
import { KnowledgeChat } from "./KnowledgeChat";
import { EvidencePreview } from "./EvidencePreview";
import { ReferenceDocuments } from "@/components/MeetingDetails/ReferenceDocuments";
import type { ProjectFilter } from "@/lib/library";
import type { IndexStatus, SearchMode } from "@/types/knowledge";

export function KnowledgeArchive({ meetings, meetingTitles = meetings, projectFilter, projectControl }: {
  meetings: { id: string; title: string }[]; meetingTitles?: { id: string; title: string }[]; projectFilter: ProjectFilter; projectControl?: ReactNode;
}) {
  const { modelConfig } = useConfig();
  const [query, setQuery] = useState("");
  const [chosenMode, setChosenMode] = useState<SearchMode | null>(null);
  const [index, setIndex] = useState<IndexStatus | null>(null);
  const [indexError, setIndexError] = useState(false);
  const [all, setAll] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [meetingQuery, setMeetingQuery] = useState("");
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [searchOpen, setSearchOpen] = useState(false);
  useEffect(() => {
    let active = true, reading = false;
    const refresh = async () => {
      if (reading) return;
      reading = true;
      try { const value = await knowledgeService.indexStatus(); if (active) { setIndex(value); setIndexError(false); } }
      catch { if (active) setIndexError(true); }
      finally { reading = false; }
    };
    void refresh();
    const timer = setInterval(() => void refresh(), 1000);
    return () => { active = false; clearInterval(timer); };
  }, []);
  const semanticReady = !!index?.semantic_enabled && index.semantic_ready > 0 && !indexError;
  const mode = semanticReady ? chosenMode ?? "hybrid" : "keyword";
  const scope = useMemo(() => libraryScope(selected, all, projectFilter, from, to), [selected, all, projectFilter, from, to]);
  const state = useKnowledgeSearch(scope, null, { provider: modelConfig?.provider, model: modelConfig?.model });
  const matchingMeetings = meetings.filter(meeting => meeting.title.toLocaleLowerCase().includes(meetingQuery.trim().toLocaleLowerCase()));
  const invalidDates = !!from && !!to && from > to;
  const sourcesReady = (all || selected.length > 0) && !invalidDates;
  const secondary = <div className="space-y-3">
    <ReferenceDocuments state={state} provider={modelConfig?.provider} />
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
  </div>;
  return <section className="flex h-full min-h-0 flex-col gap-4 overflow-hidden" aria-label="Meeting memory">
    <fieldset className="shrink-0 space-y-3">
      <legend className="mb-2 text-sm font-medium">Sources</legend>
      <div className="flex flex-wrap items-center gap-3">
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
      {!all && !selected.length && <p className="text-xs text-muted-foreground">Choose at least one meeting to begin.</p>}
      {invalidDates && <p role="alert" className="text-sm text-destructive">The start date must be on or before the end date.</p>}
    </fieldset>
    <div className="min-h-0 flex-1"><KnowledgeChat state={state} mode={mode} provider={modelConfig?.provider} model={modelConfig?.model} secondary={secondary} /></div>
    <EvidencePreview state={state} />
  </section>;
}
