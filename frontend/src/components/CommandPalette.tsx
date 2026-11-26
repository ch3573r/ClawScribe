"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { usePathname, useRouter } from "next/navigation";
import { BrainCircuit, CalendarCheck, FileText, ListChecks, Mic, Square, Upload } from "lucide-react";
import { Command, CommandGroup, CommandInput, CommandItem, CommandList } from "@/components/ui/command";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { useSidebar } from "@/components/Sidebar/SidebarProvider";
import { useRecordingState } from "@/contexts/RecordingStateContext";
import { useImportDialog } from "@/contexts/ImportDialogContext";
import { useDebouncedDiscovery } from "@/hooks/useDebouncedDiscovery";
import { useKnowledgeIndexStatus } from "@/hooks/useKnowledgeIndexStatus";
import { SETTINGS_ITEMS } from "@/components/settings/settings-sections";
import { transcriptPassageUrl } from "@/lib/knowledge-navigation";
import { libraryScope } from "@/lib/knowledge-state";
import { meetingActionUrl, type OpenActionRow } from "@/lib/meeting-follow-through";
import { knowledgeService } from "@/services/knowledgeService";

const wholeLibrary = libraryScope([], true, { tags: [], untagged: false, mode: "any" });
const maxResults = 8;

export function CommandPalette() {
  const router = useRouter();
  const pathname = usePathname();
  const { meetings, setCurrentMeeting, recordingActionReason, handleRecordingToggle } = useSidebar();
  const recording = useRecordingState();
  const { openImportDialog } = useImportDialog();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const returnFocus = useRef<HTMLElement | null>(null);
  const actionOnClose = useRef<string | (() => void) | null>(null);
  const currentCommands = useRef<readonly { id: string; action: () => void; disabled?: string | null }[]>([]);
  const { semanticReady } = useKnowledgeIndexStatus(open);
  const text = query.trim();

  useEffect(() => {
    const show = () => {
      returnFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      setQuery("");
      setOpen(true);
    };
    window.addEventListener("shortcut:open-command-palette", show);
    return () => window.removeEventListener("shortcut:open-command-palette", show);
  }, []);

  const runSearch = useCallback(async (text: string) => {
    const [transcripts, actions] = await Promise.allSettled([
      knowledgeService.search({ query: text, scope: wholeLibrary, document_ids: [], mode: semanticReady ? "hybrid" : "keyword" }),
      invoke<{ rows: OpenActionRow[] }>("list_open_reviewed_actions", { filters: { search: text }, page: 0 }),
    ]);
    return {
      passages: transcripts.status === "fulfilled" ? transcripts.value.passages.filter(passage => passage.evidence.locator.kind === "transcript").slice(0, maxResults) : [],
      actions: actions.status === "fulfilled" ? actions.value.rows.slice(0, maxResults) : [],
      incomplete: transcripts.status === "rejected" || actions.status === "rejected",
    };
  }, [semanticReady]);
  const { data, loading, error } = useDebouncedDiscovery(text, open, runSearch);
  const matches = (label: string) => label.toLowerCase().includes(text.toLowerCase());
  const titles = text ? meetings.filter(meeting => matches(meeting.title)).slice(0, maxResults) : [];
  const sections = SETTINGS_ITEMS.filter(section => matches(`Settings ${section.label}`) || (text && matches(section.desc)));

  function choose(action: string | (() => void)) {
    actionOnClose.current = action;
    setOpen(false);
  }
  function goToMeetings(tab: "archive" | "memory" | "actions") {
    if (pathname === "/meetings") window.dispatchEvent(new CustomEvent("open-meetings-tab", { detail: tab }));
    else router.push(`/meetings?tab=${tab}`);
  }
  const commands = [
    { id: "recording", label: recording.isRecording ? "Stop recording" : "Start recording", icon: recording.isRecording ? Square : Mic, action: handleRecordingToggle, disabled: recordingActionReason },
    { id: "import", label: "Import audio or video", icon: Upload, action: () => openImportDialog() },
    { id: "archive", label: "Open Archive", icon: FileText, action: () => goToMeetings("archive") },
    { id: "memory", label: "Open Meeting memory", icon: BrainCircuit, action: () => goToMeetings("memory") },
    { id: "actions", label: "Open actions", icon: ListChecks, action: () => goToMeetings("actions") },
  ];
  currentCommands.current = commands;
  const quick = commands.filter(command => matches(command.label));

  return <Dialog open={open} onOpenChange={setOpen}>
    <DialogContent className="w-[calc(100vw-2rem)] gap-0 overflow-hidden p-0 sm:max-w-2xl"
      onCloseAutoFocus={event => {
        event.preventDefault();
        returnFocus.current?.focus();
        const action = actionOnClose.current;
        actionOnClose.current = null;
        if (typeof action === "string") {
          const command = currentCommands.current.find(command => command.id === action);
          if (command && !command.disabled) command.action();
        } else {
          action?.();
        }
      }}>
      <DialogTitle className="sr-only">Command palette</DialogTitle>
      <DialogDescription className="sr-only">Search meetings, transcripts, actions and settings, or run a command.</DialogDescription>
      <Command shouldFilter={false} label="Command palette">
        <CommandInput value={query} onValueChange={setQuery} maxLength={255}
          aria-label="Search ClawScribe" placeholder="Search meetings, actions or commands…" className="h-12 pr-9" />
        <CommandList className="max-h-[min(26rem,65vh)] px-1">
          {quick.length > 0 && <CommandGroup heading="Quick commands">
            {quick.map(({ id, label, icon: Icon, disabled }) => <CommandItem key={id} value={`command:${id}`}
              onSelect={() => choose(id)} disabled={!!disabled}>
              <Icon aria-hidden="true" /><span>{label}</span>{disabled && <span className="text-xs text-muted-foreground">{disabled}</span>}
            </CommandItem>)}
          </CommandGroup>}
          {titles.length > 0 && <CommandGroup heading="Meetings">
            {titles.map(meeting => <CommandItem key={meeting.id} value={`meeting:${meeting.id}`}
              onSelect={() => choose(() => {
                setCurrentMeeting(meeting);
                router.push(`/meeting-details?${new URLSearchParams({ id: meeting.id })}`);
              })}><FileText aria-hidden="true" /><span className="truncate">{meeting.title}</span></CommandItem>)}
          </CommandGroup>}
          {!!data?.passages.length && <CommandGroup heading="Transcript passages">
            {data.passages.map(passage => <CommandItem key={passage.evidence.chunk_id} value={`passage:${passage.evidence.chunk_id}`}
              onSelect={() => choose(() => {
                const url = transcriptPassageUrl(passage.evidence);
                if (url) { setCurrentMeeting({ id: passage.meeting_id, title: passage.title }); router.push(url); }
              })}>
              <FileText aria-hidden="true" />
              <span className="min-w-0"><span className="block truncate font-medium">{passage.title}</span>
                <span className="line-clamp-2 text-xs text-muted-foreground">{passage.text}</span></span>
            </CommandItem>)}
          </CommandGroup>}
          {!!data?.actions.length && <CommandGroup heading="Open actions">
            {data.actions.map(row => <CommandItem key={`${row.meetingId}:${row.action.id}`} value={`action:${row.meetingId}:${row.action.id}`}
              onSelect={() => choose(() => {
                setCurrentMeeting({ id: row.meetingId, title: row.meetingTitle });
                router.push(meetingActionUrl(row.meetingId, row.action.id, false));
              })}>
              <CalendarCheck aria-hidden="true" /><span className="min-w-0"><span className="block truncate">{row.action.title}</span>
                <span className="block truncate text-xs text-muted-foreground">{row.meetingTitle}</span></span>
            </CommandItem>)}
          </CommandGroup>}
          {sections.length > 0 && <CommandGroup heading="Settings">
            {sections.map(({ value, label, icon: Icon }) => <CommandItem key={value} value={`settings:${value}`}
              onSelect={() => choose(() => {
                if (pathname === "/settings") window.dispatchEvent(new CustomEvent("open-settings-tab", { detail: value }));
                else router.push(`/settings?tab=${value}`);
              })}><Icon aria-hidden="true" />{label}</CommandItem>)}
          </CommandGroup>}
          {loading && <p role="status" className="p-3 text-sm text-muted-foreground">Searching saved transcripts and actions…</p>}
          {(error || data?.incomplete) && <p role="status" className="p-3 text-sm text-muted-foreground">Some results could not be loaded.</p>}
          {!loading && !error && !data?.incomplete && !quick.length && !sections.length && !titles.length && !data?.passages.length && !data?.actions.length &&
            <p role="status" className="p-3 text-sm text-muted-foreground">No matching results.</p>}
        </CommandList>
      </Command>
    </DialogContent>
  </Dialog>;
}
