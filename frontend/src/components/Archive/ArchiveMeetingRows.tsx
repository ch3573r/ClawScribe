"use client";

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ChevronRight, Ellipsis } from "lucide-react";
import Link from "next/link";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { ProjectTags } from "@/components/MeetingDetails/ProjectTags";
import { useSidebar } from "@/components/Sidebar/SidebarProvider";
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { archiveRecordingFacts, archiveReview, formatArchiveDate, visibleSelection, type ArchiveAction, type ArchiveRow } from "@/lib/archive-review";
import { MEETING_SERIES_TEMPLATE_CHANGED } from "@/lib/meetingCalendarStore";
import { ArchiveAudioRemoval, type ArchiveMeeting } from "./ArchiveAudioRemoval";
import type { BulkAction } from "./ArchiveBulkActions";

const selectionLimit = 100;
function meetingHref(id: string, action: ArchiveAction = "review") {
  const params = new URLSearchParams({ id, from: "archive" });
  if (action !== "review") params.set("archiveAction", action);
  return `/meeting-details?${params}`;
}

function actionLabel(review: ReturnType<typeof archiveReview>) {
  switch (review.action) {
    case "transcribe":
      return review.transcript === "No transcript" ? "Transcribe" : "Review audio and transcript";
    case "summary":
      return review.summary === "Summary needs retry" ? "Retry summary" : "Generate summary";
    case "files":
      return "Review recording files";
    case "review":
      return "Review meeting";
  }
}

export function useArchiveMetadata(active: boolean, revision: number, librarySize: number) {
  const [metadata, setMetadata] = useState<Map<string, ArchiveRow> | null>(null);
  const [error, setError] = useState(false);
  const request = useRef(0);
  const refresh = useCallback(async () => {
    const latest = ++request.current;
    try {
      const rows = await invoke<ArchiveRow[]>("list_archive_rows");
      if (latest !== request.current) return;
      setMetadata(new Map(rows.map(row => [row.id, row])));
      setError(false);
    } catch {
      if (latest === request.current) setError(true);
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void refresh();
    window.addEventListener("focus", refresh);
    window.addEventListener(MEETING_SERIES_TEMPLATE_CHANGED, refresh);
    const subscriptions = ["retranscription-complete", "transcript-files-synchronized", "summary-job-changed", "library-changed"].map(event => listen(event, refresh));
    for (const subscription of subscriptions) void subscription.catch(() => {});
    return () => {
      request.current += 1;
      window.removeEventListener("focus", refresh);
      window.removeEventListener(MEETING_SERIES_TEMPLATE_CHANGED, refresh);
      for (const subscription of subscriptions) void subscription.then(unlisten => unlisten()).catch(() => {});
    };
  }, [active, refresh, revision, librarySize]);

  const summaryRunning = Array.from(metadata?.values() ?? []).some(row =>
    row.summary_status !== null && ["pending", "processing", "summarizing", "regenerating"].includes(row.summary_status),
  );
  useEffect(() => {
    if (!active || !summaryRunning) return;
    const timer = window.setInterval(() => void refresh(), 5000);
    return () => window.clearInterval(timer);
  }, [active, refresh, summaryRunning]);

  return { metadata, error, refresh };
}

export function ArchiveMeetingRows({
  meetings, projectTags, status: { metadata, error, refresh }, onOpen, onBulk, onAsk, renderTitle, renderExcerpts,
}: {
  meetings: readonly ArchiveMeeting[];
  projectTags: readonly { meeting_id: string; tag: string }[];
  status: ReturnType<typeof useArchiveMetadata>;
  onOpen: (meeting: { id: string; title: string }) => void;
  onBulk: (action: BulkAction, meetings: ArchiveMeeting[], opener: HTMLElement | null) => void;
  onAsk: (ids: string[]) => void;
  renderTitle?: (meeting: ArchiveMeeting) => ReactNode;
  renderExcerpts?: (meeting: ArchiveMeeting) => ReactNode;
}) {
  const { setDeleteTarget, projectTagsLoading, projectTagsError, refreshProjectTags } = useSidebar();
  const [tagMeeting, setTagMeeting] = useState<ArchiveMeeting | null>(null);
  const [selecting, setSelecting] = useState(false);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [removal, setRemoval] = useState<ArchiveMeeting[] | null>(null);
  const removalOpener = useRef<HTMLElement | null>(null);
  const bulkOpener = useRef<HTMLButtonElement>(null);
  const filteredIds = useMemo(() => meetings.map(meeting => meeting.id), [meetings]);
  const visibleSelected = visibleSelection(selected, filteredIds);
  useEffect(() => {
    setSelected(current => {
      const visible = visibleSelection(current, filteredIds);
      return current.size === visible.size ? current : visible;
    });
  }, [filteredIds]);

  const tags = useMemo(() => {
    const result = new Map<string, string[]>();
    for (const entry of projectTags) {
      const values = result.get(entry.meeting_id);
      if (values) values.push(entry.tag);
      else result.set(entry.meeting_id, [entry.tag]);
    }
    return result;
  }, [projectTags]);

  const needsAttention: ArchiveMeeting[] = [];
  const otherMeetings: ArchiveMeeting[] = [];
  for (const meeting of meetings) {
    const row = metadata?.get(meeting.id);
    (row && archiveReview(row).needsAttention ? needsAttention : otherMeetings).push(meeting);
  }

  const stack = [...needsAttention, ...otherMeetings];
  const bulkCandidates = stack.filter(meeting => visibleSelected.has(meeting.id));
  const handleRemoved = (ids: string[]) => {
    const removed = new Set(ids);
    setSelected(current => new Set([...current].filter(id => !removed.has(id))));
    void refresh();
  };
  const closeRemoval = () => {
    setRemoval(null);
    const opener = removalOpener.current;
    removalOpener.current = null;
    window.requestAnimationFrame(() => { if (opener?.isConnected) opener.focus(); });
  };

  const renderRow = (meeting: ArchiveMeeting, isAttention: boolean) => {
    const row = metadata?.get(meeting.id);
    const review = row && archiveReview(row);
    const label = review ? actionLabel(review) : null;
    const menuId = `archive-actions-${encodeURIComponent(meeting.id)}`;
    const href = meetingHref(meeting.id);
    const taskHref = meetingHref(meeting.id, review?.action);
    const open = () => onOpen({ id: meeting.id, title: meeting.title });
    const hrefClass = "min-w-0 truncate rounded-sm font-medium hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring";

    return <li key={meeting.id} className="px-4 py-3 sm:px-5">
      <div className="flex items-center gap-3">
      {selecting && <Checkbox
        aria-label={`Select ${meeting.title}`}
        checked={visibleSelected.has(meeting.id)}
        disabled={visibleSelected.size >= selectionLimit && !visibleSelected.has(meeting.id)}
        onCheckedChange={checked => setSelected(current => {
          const next = visibleSelection(current, filteredIds);
          if (checked === true && next.size < selectionLimit) next.add(meeting.id);
          if (checked === false) next.delete(meeting.id);
          return next;
        })}
      />}
      <div className="min-w-0 flex-1">
        <Tooltip>
          <TooltipTrigger asChild>
            <Link href={href} onClick={open} className={`block w-fit max-w-full ${hrefClass}`}>{renderTitle?.(meeting) ?? meeting.title}</Link>
          </TooltipTrigger>
          <TooltipContent className="max-w-sm break-words">{meeting.title}</TooltipContent>
        </Tooltip>
        <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
          <span>{formatArchiveDate(meeting.created_at)}</span>
          {row && archiveRecordingFacts(row).map(fact => <span key={fact}>{fact}</span>)}
          {review && <>
            <span>{review.transcript}</span>
            <span className={review.summary === "Summary needs retry" ? "text-warning-foreground" : undefined}>{review.summary}</span>
            {review.warnings.map(warning => <span key={warning} className="font-medium text-warning-foreground">{warning}</span>)}
          </>}
          {tags.has(meeting.id) && <span>{tags.get(meeting.id)?.join(" · ")}</span>}
        </div>
      </div>
      {isAttention && !selecting && <Button asChild variant="outline" size="sm" className="shrink-0">
        <Link href={taskHref} onClick={open}>{label}<ChevronRight aria-hidden="true" /></Link>
      </Button>}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button id={menuId} type="button" variant="ghost" size="icon" aria-label={`Actions for ${meeting.title}`}><Ellipsis aria-hidden="true" /></Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <DropdownMenuItem asChild><Link href={href} onClick={open}>Open meeting</Link></DropdownMenuItem>
          <DropdownMenuItem disabled={projectTagsLoading} onSelect={() => {
            if (projectTagsError) void refreshProjectTags();
            else setTagMeeting(meeting);
          }}>{projectTagsError ? 'Retry loading project tags' : 'Edit project tags'}</DropdownMenuItem>
          {review && review.action !== "review" && <DropdownMenuItem asChild><Link href={taskHref} onClick={open}>{label}</Link></DropdownMenuItem>}
          {row?.has_folder && <DropdownMenuItem onSelect={() => {
            removalOpener.current = document.getElementById(menuId);
            setRemoval([meeting]);
          }}>Review saved audio</DropdownMenuItem>}
          <DropdownMenuItem className="text-destructive" onSelect={() => setDeleteTarget(meeting)}>Move to Trash…</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      </div>
      {renderExcerpts?.(meeting)}
    </li>;
  };

  return <TooltipProvider>
    <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border px-4 py-2 sm:px-5">
      {error ? <div role="status" className="flex items-center gap-2 text-sm text-muted-foreground">
        <span>Could not load archive status.</span><Button size="sm" variant="outline" onClick={() => void refresh()}>Retry</Button>
      </div> : metadata === null ? <p role="status" className="text-sm text-muted-foreground">Checking archive status…</p>
        : <span aria-live="polite" className="text-sm text-muted-foreground">{selecting ? `${visibleSelected.size} selected` : ""}</span>}
      <div className="ml-auto flex flex-wrap items-center gap-2">
        {selecting && <>
          <Button variant="ghost" size="sm" onClick={() => setSelected(new Set(stack.slice(0, selectionLimit).map(meeting => meeting.id)))}>
            {stack.length > selectionLimit ? `Select first ${selectionLimit}` : "Select visible"}
          </Button>
          <Button variant="outline" size="sm" disabled={!bulkCandidates.some(meeting => metadata?.get(meeting.id)?.has_folder)}
            onClick={event => {
              removalOpener.current = event.currentTarget;
              setRemoval(bulkCandidates);
            }}>Review selected audio</Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button ref={bulkOpener} variant="outline" size="sm" disabled={!bulkCandidates.length}>Bulk actions</Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onSelect={() => onBulk("tag", bulkCandidates, bulkOpener.current)}>Bulk tag…</DropdownMenuItem>
              <DropdownMenuItem onSelect={() => onBulk("export", bulkCandidates, bulkOpener.current)}>Bulk export…</DropdownMenuItem>
              <DropdownMenuItem onSelect={() => onAsk(bulkCandidates.map(meeting => meeting.id))}>Ask in Meeting memory</DropdownMenuItem>
              <DropdownMenuItem className="text-destructive" onSelect={() => onBulk("delete", bulkCandidates, bulkOpener.current)}>Move to Trash…</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </>}
        <Button variant="ghost" size="sm" onClick={() => {
          setSelecting(!selecting);
          setSelected(new Set());
        }}>{selecting ? "Clear selection" : "Select meetings"}</Button>
      </div>
    </div>
    {needsAttention.length > 0 && <section aria-labelledby="archive-attention-heading" className="border-b border-border">
      <h2 id="archive-attention-heading" className="border-b border-border bg-muted/50 px-4 py-2 text-sm font-semibold sm:px-5">Meetings that need attention</h2>
      <ul className="divide-y divide-border">{needsAttention.map(meeting => renderRow(meeting, true))}</ul>
    </section>}
    {otherMeetings.length > 0 && <>
      {needsAttention.length > 0 && <h2 className="border-b border-border bg-muted/50 px-4 py-2 text-sm font-semibold sm:px-5">Other meetings</h2>}
      <ul className="divide-y divide-border">{otherMeetings.map(meeting => renderRow(meeting, false))}</ul>
    </>}
    {removal && <ArchiveAudioRemoval meetings={removal} onClose={closeRemoval} onRemoved={handleRemoved} />}
    {tagMeeting && <ProjectTags key={tagMeeting.id} meetingId={tagMeeting.id} onClose={() => {
      setTagMeeting(null);
      requestAnimationFrame(() => document.getElementById(`archive-actions-${encodeURIComponent(tagMeeting.id)}`)?.focus());
    }} />}
  </TooltipProvider>;
}
