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
import { MultiSelect } from "@/components/ui/multi-select";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useSidebar } from "@/components/Sidebar/SidebarProvider";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { KnowledgeChat } from "@/components/Knowledge/KnowledgeChat";
import { EvidencePreview } from "@/components/Knowledge/EvidencePreview";
import { ReferenceDocuments } from "./ReferenceDocuments";
import { AttachReferenceButton } from "./AttachReferenceButton";
import { useKnowledgeSearch } from "@/hooks/useKnowledgeSearch";
import { libraryScope } from "@/lib/knowledge-state";
import type { ProjectFilter } from "@/lib/library";
import type { KnowledgeScope, SearchMode } from "@/types/knowledge";

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
  const [mode, setMode] = useState<SearchMode>("keyword");
  const { meetings, projectTags, projectTagsError, projectTagsLoading } =
    useSidebar();
  const tags = [...new Set(projectTags.map((row) => row.tag))];
  const scope = useMemo<KnowledgeScope>(
    () =>
      expanded
        ? libraryScope(selected, all, project)
        : { kind: "meeting", meeting_id: meetingId },
    [expanded, selected, all, project, meetingId],
  );
  const state = useKnowledgeSearch(
    scope,
    expanded ? null : { kind: "meeting", id: meetingId },
    { provider, model },
  );
  const currentState = useRef(state);
  currentState.current = state;
  const scopeKey = JSON.stringify(scope);
  const matchingMeetings = meetings.filter(meeting => meeting.title.toLocaleLowerCase().includes(meetingQuery.trim().toLocaleLowerCase()));
  const selectedFiles = state.documents.filter(row => state.selectedDocumentIds.includes(row.attachment.id));
  const meetingLabel = !expanded ? "This meeting" : all ? "All meetings" : `${selected.length} ${selected.length === 1 ? "meeting" : "meetings"}`;
  const contextLabel = `${meetingLabel}${selectedFiles.length ? ` · ${selectedFiles.length} ${selectedFiles.length === 1 ? "file" : "files"}` : ""}`;
  const contextBar = <Popover modal={false} open={contextOpen} onOpenChange={setContextOpen}>
    <PopoverTrigger asChild><Button variant="ghost" size="sm" className="h-7 max-w-full gap-1 text-xs" aria-label={`Context: ${contextLabel}`}>
      <span className="truncate">{contextLabel}</span><ChevronDown className="h-3 w-3 shrink-0" />
    </Button></PopoverTrigger>
    <PopoverContent align="start" side="top" collisionPadding={24} aria-label="Context" onEscapeKeyDown={event => event.stopPropagation()}
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
            {!selected.length && <p className="text-xs text-muted-foreground">Choose at least one meeting to begin.</p>}
          </div>}
          <Button variant="ghost" size="sm" className="text-muted-foreground" aria-expanded={filtersOpen} aria-controls={`meeting-chat-filters-${meetingId}`} onClick={() => setFiltersOpen(value => !value)}>
            <ChevronDown className={filtersOpen ? "rotate-180" : ""} />Filters
          </Button>
          {filtersOpen && <div id={`meeting-chat-filters-${meetingId}`} className="space-y-3">
            <label className="block text-xs">Projects (Ctrl or Command selects several)
              <MultiSelect multiple aria-label="Question project filters" disabled={projectTagsLoading || !!projectTagsError || project.untagged} value={project.tags}
                className="mt-1 block w-full rounded border border-input bg-background p-2 text-sm"
                onChange={event => setProject(current => ({ ...current, tags: Array.from(event.target.selectedOptions, option => option.value) }))}>
                {tags.map(tag => <option key={tag} value={tag}>{tag}</option>)}
              </MultiSelect>
            </label>
            <div className="flex flex-wrap items-center gap-3">
              <label className="text-xs">Match
                <Select value={project.mode} onValueChange={value => setProject(current => ({ ...current, mode: value as "any" | "all" }))}>
                  <SelectTrigger aria-label="Match projects" className="ml-1 inline-flex h-auto w-auto rounded border border-input bg-background p-2 text-xs shadow-none"><SelectValue /></SelectTrigger>
                  <SelectContent><SelectItem value="any">Any project</SelectItem><SelectItem value="all">All projects</SelectItem></SelectContent>
                </Select>
              </label>
              <label className="flex items-center gap-2 text-xs"><Checkbox checked={project.untagged} onCheckedChange={checked => setProject(current => ({ ...current, tags: [], untagged: checked === true }))} />Untagged only</label>
            </div>
            {projectTagsLoading && <p role="status" className="text-xs text-muted-foreground">Loading projects…</p>}
            {projectTagsError && <p role="alert" className="text-xs text-destructive">Project filters are unavailable. Retry from Meetings.</p>}
          </div>}
          <p className="text-xs text-muted-foreground">Other meetings use a separate saved conversation. Choose This meeting only to return to its history.</p>
        </>}
      </section>
      <ReferenceDocuments compact state={state} provider={provider} attachMeetingId={meetingId} attachmentBusy={attachmentBusy} disabled={referenceBusy} onBusyChange={setReferenceBusy} onAddFile={() => attachmentButton.current?.click()}
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
  const chatContent = (          <KnowledgeChat state={state} mode={mode} provider={provider} model={model} title="Chat with this meeting" compact
            headerActions={<Tooltip><TooltipTrigger asChild><Button variant="ghost" size="icon" className="h-7 w-7" aria-label="Close meeting chat" onClick={closeChat}><X className="h-4 w-4" /></Button></TooltipTrigger><TooltipContent>Close meeting chat</TooltipContent></Tooltip>}
            contextBar={contextBar}
            composerStart={<AttachReferenceButton variant="icon" buttonRef={attachmentButton} meetingId={meetingId} scopeKey={JSON.stringify(scope)} disabled={referenceBusy || attachmentBusy} onBusyChange={setAttachmentBusy} onAttached={() => state.reloadDocuments()} />}
            attachments={selectedFiles.length > 0 && <div className="flex flex-wrap gap-1" aria-label="Selected reference files">
              {selectedFiles.slice(0, 2).map(row => <span key={row.attachment.id} className="inline-flex max-w-full items-center gap-1 rounded-full bg-muted pl-2 text-xs">
                <span className="truncate">{row.attachment.display_name}</span>
                <Tooltip><TooltipTrigger asChild><Button type="button" variant="ghost" size="icon" className="h-6 w-6 shrink-0 rounded-full" aria-label={`Remove ${row.attachment.display_name} from this question`} onClick={() => state.selectDocument(row.attachment.id, false)}><X className="h-3 w-3" /></Button></TooltipTrigger><TooltipContent>Remove from this question</TooltipContent></Tooltip>
              </span>)}
              {selectedFiles.length > 2 && <Button type="button" variant="ghost" size="sm" className="h-6 px-2 text-xs" aria-label={`Manage ${selectedFiles.length - 2} more reference ${selectedFiles.length === 3 ? 'file' : 'files'} in Context`} onClick={() => setContextOpen(true)}>+{selectedFiles.length - 2} {selectedFiles.length === 3 ? 'file' : 'files'}</Button>}
            </div>}
            suggestions={["What did we decide?", "List the action items", "Summarize the key points"]}
            headerMenuExtra={<>
              <DropdownMenuLabel>Search mode</DropdownMenuLabel>
              <DropdownMenuRadioGroup value={mode} onValueChange={value => { state.cancel(); setMode(value as SearchMode); }}>
                <DropdownMenuRadioItem value="keyword">Keyword</DropdownMenuRadioItem>
                <DropdownMenuRadioItem value="hybrid">Semantic + keyword</DropdownMenuRadioItem>
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
        onNavigate={() => {
          setOpen(false);
          setContextOpen(false);
          state.cancel();
        }}
      />
    </>
  );
}
