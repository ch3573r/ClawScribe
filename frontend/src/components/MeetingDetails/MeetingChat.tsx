"use client";
import { useMemo, useState } from "react";
import { ChevronDown, MessageSquare, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { MultiSelect } from "@/components/ui/multi-select";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useSidebar } from "@/components/Sidebar/SidebarProvider";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { KnowledgeChat } from "@/components/Knowledge/KnowledgeChat";
import { EvidencePreview } from "@/components/Knowledge/EvidencePreview";
import { ReferenceDocuments } from "./ReferenceDocuments";
import { useKnowledgeSearch } from "@/hooks/useKnowledgeSearch";
import { libraryScope } from "@/lib/knowledge-state";
import type { ProjectFilter } from "@/lib/library";
import type { KnowledgeScope, SearchMode } from "@/types/knowledge";

export function MeetingChat({
  meetingId,
  provider,
  model,
}: {
  meetingId: string;
  provider?: string;
  model?: string;
}) {
  const [open, setOpen] = useState(false);
  const [sourcesOpen, setSourcesOpen] = useState(false);
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
  return (
    <>
      <Popover
        modal={false}
        open={open}
        onOpenChange={(value) => {
          setOpen(value);
          if (!value) state.cancel();
          else void state.reloadHistory();
        }}
      >
        <PopoverTrigger asChild>
          <Button variant="ghost"
            aria-label="Chat with this meeting"
            className="gap-0 p-0 [&_svg]:size-5 fixed bottom-6 right-6 z-40 flex h-12 w-12 items-center justify-center rounded-full bg-primary text-primary-foreground hover:bg-primary hover:text-primary-foreground shadow-lg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            <MessageSquare className="h-5 w-5" />
          </Button>
        </PopoverTrigger>
        <PopoverContent side="top" align="end" sideOffset={12} collisionPadding={24} aria-label="Chat with this meeting"
          onInteractOutside={event => event.preventDefault()}
          className="flex h-[32rem] max-h-[calc(100vh-6rem)] w-[26rem] max-w-[calc(100vw-3rem)] flex-col overflow-hidden rounded-xl border-border bg-card p-0 text-foreground shadow-2xl">
          <KnowledgeChat state={state} mode={mode} provider={provider} model={model} title="Chat with this meeting" compact
            headerActions={<Button variant="ghost" size="icon" className="h-7 w-7" aria-label="Close meeting chat" onClick={() => {setOpen(false);state.cancel();}}><X className="h-4 w-4" /></Button>}
            secondary={<div className="space-y-3">
              <Button variant="ghost" size="sm" className="text-muted-foreground" aria-expanded={sourcesOpen} aria-controls={`meeting-chat-scope-${meetingId}`} onClick={() => setSourcesOpen(value => !value)}><ChevronDown className={sourcesOpen ? 'rotate-180' : ''} />Question scope</Button>
              {sourcesOpen && <div id={`meeting-chat-scope-${meetingId}`} className="space-y-3">
          <fieldset className="space-y-3 rounded-lg border border-border p-3">
            <legend className="sr-only">Question scope</legend>
            <label className="flex items-center gap-2 text-sm">
              <Checkbox
                className="h-[13px] w-[13px]"
                checked={expanded}
                onCheckedChange={(checked) => setExpanded(checked === true)}
              />
              Include other saved meetings
            </label>
            {expanded && (
              <>
                <label className="flex items-center gap-2 text-sm">
                  <Checkbox
                    className="h-[13px] w-[13px]"
                    checked={all}
                    onCheckedChange={(checked) => setAll(checked === true)}
                  />
                  Search all saved meetings
                </label>
                {!all && (
                  <div className="space-y-2">
                    {meetings.map((meeting) => (
                      <label
                        key={meeting.id}
                        className="flex items-start gap-2 text-sm"
                      >
                        <Checkbox
                          className="mt-1 h-[13px] w-[13px]"
                          checked={selected.includes(meeting.id)}
                          onCheckedChange={(checked) =>
                            setSelected((ids) =>
                              checked === true
                                ? [...ids, meeting.id]
                                : ids.filter((id) => id !== meeting.id),
                            )
                          }
                        />
                        <span className="break-words">{meeting.title}</span>
                      </label>
                    ))}
                  </div>
                )}
                <label className="block text-xs">
                  Projects (Ctrl or Command selects several)
                  <MultiSelect
                    multiple
                    aria-label="Question project filters"
                    disabled={
                      projectTagsLoading ||
                      !!projectTagsError ||
                      project.untagged
                    }
                    value={project.tags}
                    className="mt-1 block w-full rounded border border-input bg-background p-2 text-sm"
                    onChange={(e) =>
                      setProject((current) => ({
                        ...current,
                        tags: Array.from(
                          e.target.selectedOptions,
                          (option) => option.value,
                        ),
                      }))
                    }
                  >
                    {tags.map((tag) => (
                      <option key={tag} value={tag}>
                        {tag}
                      </option>
                    ))}
                  </MultiSelect>
                </label>
                <div className="flex flex-wrap items-center gap-3">
                  <label className="text-xs">
                    Match{" "}
                    <Select
                      value={project.mode}
                      onValueChange={(value) =>
                        setProject((current) => ({
                          ...current,
                          mode: value as "any" | "all",
                        }))
                      }
                    >
                      <SelectTrigger className="inline-flex h-auto w-auto rounded border border-input bg-background p-2 text-xs shadow-none"><SelectValue /></SelectTrigger>
                      <SelectContent>
                        <SelectItem value="any">Any project</SelectItem>
                        <SelectItem value="all">All projects</SelectItem>
                      </SelectContent>
                    </Select>
                  </label>
                  <label className="flex items-center gap-2 text-xs">
                    <Checkbox
                      className="h-[13px] w-[13px]"
                      checked={project.untagged}
                      onCheckedChange={(checked) =>
                        setProject((current) => ({
                          ...current,
                          tags: [],
                          untagged: checked === true,
                        }))
                      }
                    />
                    Untagged only
                  </label>
                </div>
                {projectTagsError && (
                  <p role="alert" className="text-xs text-destructive">
                    Project filters are unavailable. Retry from Meetings.
                  </p>
                )}
                <p className="text-xs text-muted-foreground">
                  An empty meeting selection searches no meetings.
                </p>
              </>
            )}
            <label className="flex items-center gap-2 text-xs">
              Retrieval{" "}
              <Select
                value={mode}
                onValueChange={(value) => {
                  state.cancel();
                  setMode(value as SearchMode);
                }}
              >
                <SelectTrigger className="inline-flex h-auto w-auto rounded border border-input bg-background p-2 text-xs shadow-none"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="keyword">Keyword</SelectItem>
                  <SelectItem value="hybrid">Semantic + keyword</SelectItem>
                </SelectContent>
              </Select>
            </label>
          </fieldset>
          {expanded && (
            <p className="text-xs text-muted-foreground">
              Cross-meeting questions use a separate saved library conversation.
              The first question starts one automatically. Turn off this scope to return to this
              meeting’s history.
            </p>
          )}
              </div>}
              <ReferenceDocuments state={state} provider={provider} attachMeetingId={meetingId} />
            </div>}
          />
        </PopoverContent>
      </Popover>
      <EvidencePreview
        state={state}
        onNavigate={() => {
          setOpen(false);
          state.cancel();
        }}
      />
    </>
  );
}
