"use client";
import { useMemo, useRef, useState, useEffect } from "react";
import { createPortal } from 'react-dom';
import { Tooltip, TooltipTrigger, TooltipContent } from '@/components/ui/tooltip';
import { ChevronDown, MessageSquare, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { ScrollArea } from "@/components/ui/scroll-area";
import { DropdownMenuLabel, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { useSidebar } from "@/components/Sidebar/SidebarProvider";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { ProjectFilterMenu } from "@/components/Knowledge/ProjectFilterMenu";
import { KnowledgeChat } from "@/components/Knowledge/KnowledgeChat";
import { EvidencePreview } from "@/components/Knowledge/EvidencePreview";
import { ReferenceDocuments } from "./ReferenceDocuments";
import { AttachReferenceButton } from "./AttachReferenceButton";
import { useKnowledgeSearch } from "@/hooks/useKnowledgeSearch";
import { useKnowledgeIndexStatus } from "@/hooks/useKnowledgeIndexStatus";
import { archiveDateKey } from "@/lib/archive-review";
import { useConfig } from "@/contexts/ConfigContext";
import { classifySummaryProvider } from "@/lib/privacy";
import { knowledgeSearchMode, libraryScope, scopeReady } from "@/lib/knowledge-state";
import { matchesProjectFilter, type ProjectFilter } from "@/lib/library";
import type { DocumentAttachment, KnowledgeScope, MeetingFilter, SearchMode } from "@/types/knowledge";

export function MeetingChat({
  meetingId,
  provider,
  model, dockTarget, onDockOpenChange,
}: {
  meetingId: string;
  provider?: string;
  model?: string;
  dockTarget?: HTMLDivElement | null;
  onDockOpenChange?: (open: boolean) => void;
}) {
  const { modelConfig } = useConfig();
  const destination = classifySummaryProvider(provider, {
    ollamaEndpoint: modelConfig?.ollamaEndpoint, customEndpoint: modelConfig?.customOpenAIEndpoint,
  });
  const [wide, setWide] = useState(false);
  const [open, setOpen] = useState(false);
  const docked = Boolean(wide && dockTarget);
  useEffect(() => { onDockOpenChange?.(docked && open); return () => onDockOpenChange?.(false); }, [docked, open, onDockOpenChange]);
  // Move a stable portal host between the two surfaces. Resizing must not
  // remount the composer and discard an unsent question or an attachment picker.
  const [contentHost] = useState(() => typeof document === 'undefined' ? null : document.createElement('div'));
  const [popoverTarget, setPopoverTarget] = useState<HTMLDivElement | null>(null);
  const launcherRef = useRef<HTMLButtonElement>(null);
  const focusOnAttach = useRef(false);
  const latestPlacement = useRef({ docked, open });
  latestPlacement.current = { docked, open };
  const focusedComposer = useRef<{ element: HTMLTextAreaElement; start: number | null; end: number | null; direction: 'forward' | 'backward' | 'none' | null } | null>(null);
  useEffect(() => {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return;
    const media = window.matchMedia('(min-width: 1280px)');
    const update = () => {
      // Capture before React unmounts the old popover; the browser can blur
      // its textarea before passive effect cleanup sees the detached host.
      const composer = contentHost?.querySelector<HTMLTextAreaElement>('textarea[id^="knowledge-question-"]');
      if (composer && contentHost?.ownerDocument.activeElement === composer) {
        focusedComposer.current = { element: composer, start: composer.selectionStart, end: composer.selectionEnd, direction: composer.selectionDirection };
      }
      setWide(media.matches);
    };
    update(); media.addEventListener('change', update);
    return () => media.removeEventListener('change', update);
  }, [contentHost]);
  useEffect(() => {
    const parent = docked ? dockTarget : popoverTarget;
    if (!open || !parent || !contentHost) return;
    contentHost.className = 'flex h-full min-h-0 flex-1 flex-col';
    parent.appendChild(contentHost);
    const restore = focusedComposer.current;
    const focus = () => {
      if (!latestPlacement.current.open) return;
      const composer = contentHost.querySelector<HTMLTextAreaElement>('textarea[id^="knowledge-question-"]');
      const target = restore?.element ?? (focusOnAttach.current ? composer : null);
      if (target && contentHost.contains(target)) {
        target.focus({ preventScroll: true });
        if (restore && restore.start !== null && restore.end !== null) target.setSelectionRange(restore.start, restore.end, restore.direction ?? undefined);
      }
      focusedComposer.current = null;
      focusOnAttach.current = false;
    };
    // The parent must first make the dock visible after onDockOpenChange.
    const frame = typeof window.requestAnimationFrame === 'function' ? window.requestAnimationFrame(focus) : undefined;
    if (frame === undefined) focus();
    return () => {
      if (frame !== undefined) window.cancelAnimationFrame(frame);
      const composer = contentHost.querySelector<HTMLTextAreaElement>('textarea[id^="knowledge-question-"]');
      if (composer && contentHost.ownerDocument.activeElement === composer) {
        focusedComposer.current = { element: composer, start: composer.selectionStart, end: composer.selectionEnd, direction: composer.selectionDirection };
      }
      contentHost.remove();
    };
  }, [open, docked, dockTarget, popoverTarget, contentHost]);
  const [contextOpen, setContextOpen] = useState(false);
  const [meetingQuery, setMeetingQuery] = useState("");
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [attachmentBusy, setAttachmentBusy] = useState(false);
  const [referenceBusy, setReferenceBusy] = useState(false);
  const [referenceFailure, setReferenceFailure] = useState<{scope: string; message: string} | null>(null);
  const attachmentButton = useRef<HTMLButtonElement>(null);
  const [expanded, setExpanded] = useState(false);
  const [selected, setSelected] = useState<string[]>([meetingId]);
  const [all, setAll] = useState(false);
  const [project, setProject] = useState<ProjectFilter>({
    tags: [],
    untagged: false,
    mode: "any",
  });
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [chosenMode, setChosenMode] = useState<SearchMode | null>(null);
  const { semanticReady, indexError } = useKnowledgeIndexStatus(open);
  const mode = knowledgeSearchMode(chosenMode, semanticReady);
  const { meetings, projectTags } = useSidebar();
  const scope = useMemo<KnowledgeScope>(
    () =>
      expanded
        ? libraryScope(selected, all, project, from, to)
        : { kind: "meeting", meeting_id: meetingId },
    [expanded, selected, all, project, meetingId, from, to],
  );
  const state = useKnowledgeSearch(
    scope,
    expanded ? null : { kind: "meeting", id: meetingId },
    { provider, model },
    {
      draftMeetingIds: selected,
      restoreScope: (filter: MeetingFilter | null) => {
        setAll(filter?.all_meetings ?? false);
        setSelected(filter?.meeting_ids ?? []);
        setProject({ tags: filter?.tags ?? [], untagged: filter?.untagged ?? false, mode: filter?.tag_mode ?? "any" });
        setFrom(filter?.from ?? "");
        setTo(filter?.to ?? "");
      },
    },
  );
  const currentState = useRef(state);
  currentState.current = state;
  const scopeKey = JSON.stringify(scope);
  const matchingMeetings = meetings.filter(meeting => meeting.title.toLocaleLowerCase().includes(meetingQuery.trim().toLocaleLowerCase()));
  const attachmentMeetings = meetings.filter(meeting => {
    const date = archiveDateKey(meeting.created_at);
    return matchesProjectFilter(meeting.id, project, projectTags) &&
      !(from && to && from > to) && (!from || (!!date && date >= from)) && (!to || (!!date && date <= to));
  });
  const attachmentMeetingId = !expanded ? meetingId
    : !all && selected.length === 1 && attachmentMeetings.some(meeting => meeting.id === selected[0]) ? selected[0] : undefined;
  const attached = (id: string, attachment: DocumentAttachment) => {
    if (expanded && !all && (selected.length > 0 || !scopeReady(scope))) {
      setSelected(ids => [...new Set([...ids, id])]);
    }
    state.selectImportedDocument(attachment);
  };
  const selectedFiles = state.selectedFiles;
  const meetingLabel = !expanded ? "This meeting" : all ? "All meetings" : !selected.length && scopeReady(scope) ? "Filtered meetings" : `${selected.length} ${selected.length === 1 ? "meeting" : "meetings"}`;
  const contextLabel = `${meetingLabel}${selectedFiles.length ? ` · ${selectedFiles.length} ${selectedFiles.length === 1 ? "file" : "files"}` : ""}`;
  const contextBar = <Popover modal={false} open={contextOpen} onOpenChange={setContextOpen}>
    <PopoverTrigger asChild><Button variant="ghost" size="sm" className="h-7 max-w-full gap-1 text-xs" aria-label={`Context: ${contextLabel}`}>
      <span className="truncate">{contextLabel}</span><ChevronDown className="h-3 w-3 shrink-0" />
    </Button></PopoverTrigger>
    <PopoverContent align="start" side="top" collisionPadding={24} aria-label="Context" onEscapeKeyDown={event => event.stopPropagation()}
      onInteractOutside={event => { if (event.target instanceof Element && event.target.closest("[data-project-filter-menu]")) event.preventDefault(); }}
      className="max-h-[60vh] w-[24rem] max-w-[calc(100vw-3rem)] space-y-4 overflow-y-auto overscroll-contain bg-card text-foreground">
      <section className="space-y-3" aria-label="Meetings">
        <h3 className="text-sm font-medium">Meetings</h3>
        <div className="flex flex-wrap gap-1" role="group" aria-label="Meeting scope">
          <Button variant="outline" size="sm" className="aria-pressed:bg-accent aria-pressed:text-accent-foreground" aria-pressed={!expanded} onClick={() => setExpanded(false)}>This meeting only</Button>
          <Button variant="outline" size="sm" className="aria-pressed:bg-accent aria-pressed:text-accent-foreground" aria-pressed={expanded} onClick={() => setExpanded(true)}>Other saved meetings</Button>
        </div>
        {expanded && <>
          <label className="flex items-center gap-2 text-sm"><Switch checked={all} onCheckedChange={setAll} />Search all saved meetings</label>
          {!all && <div className="space-y-2">
            <Input aria-label="Find a meeting" placeholder="Find a meeting…" value={meetingQuery} onChange={event => setMeetingQuery(event.target.value)} />
            <ScrollArea className="h-56"><div className="space-y-3 pr-3">
              {matchingMeetings.map(meeting => <label key={meeting.id} className="flex items-start gap-2 text-sm">
                <Checkbox className="mt-0.5" checked={selected.includes(meeting.id)} onCheckedChange={checked => setSelected(ids => checked === true ? [...new Set([...ids, meeting.id])] : ids.filter(id => id !== meeting.id))} />
                <span className="break-words">{meeting.title}</span>
              </label>)}
              {!matchingMeetings.length && <p className="text-xs text-muted-foreground">No matching meetings.</p>}
            </div></ScrollArea>
            {!scopeReady(scope) && <p className="text-xs text-muted-foreground">Choose meetings or filter by project or date.</p>}
          </div>}
          <Button variant="ghost" size="sm" className="text-muted-foreground" aria-expanded={filtersOpen} aria-controls={`meeting-chat-filters-${meetingId}`} onClick={() => setFiltersOpen(value => !value)}>
            <ChevronDown className={filtersOpen ? "rotate-180" : ""} />Filters
          </Button>
          {filtersOpen && <div id={`meeting-chat-filters-${meetingId}`} className="space-y-3">
            <div className="grid grid-cols-2 gap-3">
              <label className="space-y-1 text-xs">From<Input type="date" value={from} onChange={event => setFrom(event.target.value)} /></label>
              <label className="space-y-1 text-xs">To<Input type="date" value={to} onChange={event => setTo(event.target.value)} /></label>
            </div>
            {from && to && from > to && <p role="alert" className="text-xs text-destructive">The start date must be on or before the end date.</p>}
            <ProjectFilterMenu filter={project} setFilter={setProject} />
          </div>}
          <p className="text-xs text-muted-foreground">Other meetings use a separate saved conversation. Choose This meeting only to return to its history.</p>
        </>}
      </section>
      <ReferenceDocuments compact state={state} provider={provider} attachMeetingId={attachmentMeetingId} attachmentBusy={attachmentBusy} disabled={referenceBusy} onBusyChange={setReferenceBusy} onAddFile={() => attachmentButton.current?.click()}
        actionError={referenceFailure?.scope === scopeKey ? referenceFailure.message : ''} onActionError={message => setReferenceFailure({scope: scopeKey, message})}
        onDocumentsChanged={(id, action) => {
          const current = currentState.current;
          const sameScope = JSON.stringify(current.scope) === scopeKey;
          if (!sameScope && !current.documents.some(row => row.attachment.id === id)) return;
          if (action !== 'retry' && (sameScope || (action === 'delete' && current.selectedDocumentIds.includes(id)))) {
            current.selectDocument(id, false);
            current.reloadDocuments();
          } else current.refreshDocuments();
        }} />
    </PopoverContent>
  </Popover>;
  const closeChat = () => {
    focusOnAttach.current = false;
    focusedComposer.current = null;
    setOpen(false); setContextOpen(false); state.cancel();
    launcherRef.current?.focus({ preventScroll: true });
  };
  const chatContent = (          <KnowledgeChat state={state} mode={mode} provider={provider} model={model} destinationLabel={destination.label} title="Chat with this meeting" compact
            headerActions={<Tooltip><TooltipTrigger asChild><Button variant="ghost" size="icon" className="h-7 w-7" aria-label="Close meeting chat" onClick={closeChat}><X className="h-4 w-4" /></Button></TooltipTrigger><TooltipContent>Close meeting chat</TooltipContent></Tooltip>}
            contextBar={contextBar}
            attachmentBusy={attachmentBusy} onManageAttachments={() => setContextOpen(true)}
            composerStart={<AttachReferenceButton variant="icon" buttonRef={attachmentButton} meetingId={attachmentMeetingId} meetings={expanded ? attachmentMeetings : undefined} scopeKey={JSON.stringify([scope, state.owner])} disabled={referenceBusy || attachmentBusy} onBusyChange={setAttachmentBusy} onAttached={attached} />}
            suggestions={["What did we decide?", "List the action items", "Summarize the key points"]}
            headerMenuExtra={<>
              <DropdownMenuLabel>Search mode</DropdownMenuLabel>
              <DropdownMenuRadioGroup value={mode} onValueChange={value => { state.cancel(); setChosenMode(value as SearchMode); }}>
                <DropdownMenuRadioItem value="keyword">Keyword</DropdownMenuRadioItem>
                <DropdownMenuRadioItem value="hybrid" disabled={!semanticReady}>{indexError ? "Semantic search unavailable" : "Semantic + keyword"}</DropdownMenuRadioItem>
              </DropdownMenuRadioGroup>
              <DropdownMenuSeparator />
            </>}
          />
  );
  const chatSurface = <div className="flex h-full min-h-0 flex-1 flex-col" onKeyDown={event => {
    if (docked && event.key === 'Escape' && !event.defaultPrevented && !event.nativeEvent?.isComposing && !contextOpen) {
      event.preventDefault(); event.stopPropagation(); closeChat();
    }
  }}>{chatContent}</div>;
  return (
    <>
      <Popover
        modal={false}
        open={open}
        onOpenChange={(value) => {
          if (!value) closeChat();
          else { focusOnAttach.current = true; setOpen(true); void state.reloadHistory(); }
        }}
      >
        <PopoverTrigger asChild>
          <Button ref={launcherRef} variant="ghost"
            aria-label="Chat with this meeting"
            aria-expanded={open}
            className="shrink-0 gap-2 text-xs"
          >
            <MessageSquare className="h-4 w-4" />Chat with this meeting
          </Button>
        </PopoverTrigger>
        {!docked && <PopoverContent side="top" align="end" sideOffset={12} collisionPadding={24} aria-label="Chat with this meeting"
          onOpenAutoFocus={event => event.preventDefault()}
          onCloseAutoFocus={event => { if (latestPlacement.current.docked && latestPlacement.current.open) event.preventDefault(); }}
          onInteractOutside={event => event.preventDefault()}
          onEscapeKeyDown={event => { if (contextOpen) event.preventDefault(); }}
          className="flex h-[min(36rem,calc(100vh-6rem))] max-h-[calc(100vh-6rem)] w-[26rem] max-w-[calc(100vw-3rem)] flex-col overflow-hidden rounded-xl border-border bg-card p-0 text-foreground shadow-2xl">
          {contentHost ? <div ref={setPopoverTarget} className="flex min-h-0 flex-1 flex-col" /> : chatSurface}
        </PopoverContent>}
      </Popover>
      {contentHost && open ? createPortal(chatSurface, contentHost)
        : docked && open && dockTarget && createPortal(chatSurface, dockTarget)}
      <EvidencePreview
        state={state}
        onNavigate={({ meetingId: targetMeeting, play }) => {
          if (docked && targetMeeting === meetingId && !play) return;
          setOpen(false);
          setContextOpen(false);
          state.cancel();
        }}
      />
    </>
  );
}
