"use client";

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { ChevronDown } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { PageSection } from "@/components/ui/page-section";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { PlannerExportPreview } from "@/components/MeetingDetails/PlannerExportPreview";
import { ToDoExportPreview } from "@/components/MeetingDetails/ToDoExportPreview";
import { formatArchiveDate } from "@/lib/archive-review";
import { getExportDestinations, hasPlannerDestination, hasToDoDestination, type ExportDestinations } from "@/lib/exportDestinations";
import {
  actionRowKey, groupSelectedActions, meetingActionUrl, MICROSOFT_EXPORT_SETTINGS, REVIEWED_ACTIONS_CHANGED,
  type ActionExportGroup, type OpenActionRow,
} from "@/lib/meeting-follow-through";
import { editReviewedActions, type ReviewedActionEdit } from "@/services/meetingActionsService";
import type { ExportReport } from "@/services/microsoftExportService";

interface ActionPage {
  rows: OpenActionRow[];
  series: { id: string; label: string; createdAt: string }[];
  hasMore: boolean;
  skippedMeetings: number;
}

type ExportReview = {
  kind: "planner" | "todo";
  groups: ActionExportGroup[];
  destination: ExportDestinations;
};

const initialFilters = { search: "", status: "open", owner: "", dueFrom: "", dueTo: "", series: "" };
let previousFilters = initialFilters;

export function OpenMeetingActions() {
  const router = useRouter();
  const [filters, setFilters] = useState(previousFilters);
  const [result, setResult] = useState<ActionPage | null>(null);
  const [page, setPage] = useState(0);
  const [revision, setRevision] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [selected, setSelected] = useState(() => new Set<string>());
  const [saving, setSaving] = useState(false);
  const pending = useRef<Promise<unknown> | null>(null);
  const [review, setReview] = useState<ExportReview | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  function refresh() {
    setLoading(true);
    setRevision(value => value + 1);
  }

  function filter(key: keyof typeof filters, value: string) {
    setLoading(true);
    setSelected(new Set());
    setFilters(current => previousFilters = { ...current, [key]: value });
    setPage(0);
  }

  function changePage(next: number) {
    setLoading(true);
    setSelected(new Set());
    setPage(next);
  }

  useEffect(() => {
    const subscription = listen(REVIEWED_ACTIONS_CHANGED, refresh);
    return () => { void subscription.then(unlisten => unlisten()).catch(() => {}); };
  }, []);

  useEffect(() => {
    let active = true;
    setError("");
    const timer = window.setTimeout(() => {
      const args = Object.fromEntries(Object.entries(filters).map(([key, value]) => [key, value || null]));
      void invoke<ActionPage>("list_open_reviewed_actions", { filters: args, page }).then(rows => {
        if (!active) return;
        if (page > 0 && rows.rows.length === 0) {
          changePage(page - 1);
          return;
        }
        setResult(rows);
        setLoading(false);
      }).catch(failure => {
        if (active) {
          setError(typeof failure === "string" ? failure : "Could not load actions.");
          setLoading(false);
        }
      });
    }, 180);
    return () => { active = false; window.clearTimeout(timer); };
  }, [filters, page, revision]);

  const available = loading || error ? [] : result?.rows ?? [];
  const groups = groupSelectedActions(available, selected);
  const selectedCount = groups.reduce((sum, group) => sum + group.actionIds.length, 0);

  async function saveRow(row: OpenActionRow, edit: ReviewedActionEdit) {
    if (pending.current) return false;
    setSaving(true);
    try {
      const request = editReviewedActions(row.meetingId, [edit]);
      pending.current = request;
      await request;
      if (edit.done) {
        const key = actionRowKey(row.meetingId, row.action.id);
        setSelected(current => {
          const next = new Set(current);
          next.delete(key);
          return next;
        });
      }
      refresh();
      return true;
    } catch (failure) {
      toast.error("Could not save action", { description: String(failure) });
      return false;
    } finally {
      pending.current = null;
      setSaving(false);
    }
  }

  async function toggleDone(row: OpenActionRow, checked: boolean) {
    const id = row.action.id;
    const key = actionRowKey(row.meetingId, id);
    const saved = await saveRow(row, { id, done: { from: row.action.done, to: checked } });
    if (!saved) return;
    searchRef.current?.focus();
    if (checked) toast.success("Action marked done", {
      id: key,
      action: { label: "Undo", onClick: async () => {
        await pending.current?.catch(() => {});
        await saveRow(row, { id, done: { from: true, to: false } });
      } },
    });
    else toast.dismiss(key);
  }

  function openExport(kind: ExportReview["kind"]) {
    if (saving || !selectedCount) return;
    const destination = getExportDestinations();
    const ready = kind === "planner" ? hasPlannerDestination(destination) : hasToDoDestination(destination);
    if (!ready) {
      const label = kind === "planner" ? "Planner" : "Microsoft To Do";
      toast.info(`Set a ${label} destination`, {
        action: { label: "Open settings", onClick: () => router.push(MICROSOFT_EXPORT_SETTINGS) },
      });
      return;
    }
    setReview({ kind, groups, destination });
  }

  function reportExport(report: ExportReport) {
    const label = review?.kind === "planner" ? "Planner" : "Microsoft To Do";
    if (report.overall === "succeeded") toast.success(`${label} export complete`);
    else toast.warning(`${label} export finished with issues`, {
      description: "Open export history from the source meeting to review the result.",
    });
  }

  return <PageSection title="Open actions" actions={
    <Button variant="outline" size="sm" disabled={loading || saving} onClick={refresh}>Refresh</Button>
  }>
    <div className="grid gap-3 border-b border-border p-5 sm:grid-cols-3">
      <div className="min-w-0 space-y-1">
        <Label htmlFor="follow-up-search">Search actions</Label>
        <Input ref={searchRef} id="follow-up-search" type="search" value={filters.search} maxLength={255}
          placeholder="Action, meeting or owner" onChange={event => filter("search", event.target.value)} />
      </div>
      <div className="space-y-1">
        <Label htmlFor="follow-up-status">Status</Label>
        <Select value={filters.status} onValueChange={value => filter("status", value)}>
          <SelectTrigger id="follow-up-status"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="open">Open</SelectItem>
            <SelectItem value="overdue">Overdue</SelectItem>
            <SelectItem value="dueSoon">Due soon</SelectItem>
            <SelectItem value="completed">Completed</SelectItem>
          </SelectContent>
        </Select>
      </div>
      <div className="min-w-0 space-y-1">
        <Label htmlFor="follow-up-owner">Owner</Label>
        <Input id="follow-up-owner" value={filters.owner} maxLength={160} placeholder="All owners"
          onChange={event => filter("owner", event.target.value)} />
      </div>
      <div className="space-y-1">
        <Label htmlFor="follow-up-from">Due from</Label>
        <Input id="follow-up-from" type="date" value={filters.dueFrom} onChange={event => filter("dueFrom", event.target.value)} />
      </div>
      <div className="space-y-1">
        <Label htmlFor="follow-up-to">Due through</Label>
        <Input id="follow-up-to" type="date" value={filters.dueTo} onChange={event => filter("dueTo", event.target.value)} />
      </div>
      <div className="min-w-0 space-y-1">
        <Label htmlFor="follow-up-series">Recurring series</Label>
        <Select value={filters.series || "__all__"} onValueChange={value => filter("series", value === "__all__" ? "" : value)}>
          <SelectTrigger id="follow-up-series"><SelectValue /></SelectTrigger>
          <SelectContent>
            <SelectItem value="__all__">All meetings</SelectItem>
            {result?.series.map(series => <SelectItem key={series.id} value={series.id}>{series.label} · {formatArchiveDate(series.createdAt)}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
    </div>
    <div className="space-y-3 p-5" aria-busy={loading}>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      {loading && <p role="status" className="text-sm text-muted-foreground">Loading actions…</p>}
      {!loading && !error && !available.length && <p className="text-sm text-muted-foreground">No actions match these filters.</p>}
      {!!result?.skippedMeetings && <p role="status" className="text-sm text-muted-foreground">Some saved actions could not be read. Open their meetings to review them.</p>}
      {available.length > 0 && filters.status !== "completed" && <div className="flex flex-wrap items-center justify-between gap-2">
        <Button variant="ghost" size="sm" disabled={saving} onClick={() => {
          setSelected(selectedCount === available.length ? new Set() : new Set(available.map(row => actionRowKey(row.meetingId, row.action.id))));
        }}>{selectedCount === available.length ? "Clear selection" : "Select page"}</Button>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button size="sm" disabled={!selectedCount || saving}>Review export{selectedCount > 0 && ` (${selectedCount})`} <ChevronDown /></Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem onSelect={() => openExport("planner")}>Planner</DropdownMenuItem>
            <DropdownMenuItem onSelect={() => openExport("todo")}>Microsoft To Do</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>}
      {result && !error && <ul className="divide-y divide-border" hidden={loading}>
        {result.rows.map(row => {
          const key = actionRowKey(row.meetingId, row.action.id);
          return <ActionListRow key={key} row={row} selected={selected.has(key)} disabled={saving || loading}
            onSelect={value => setSelected(current => {
              const next = new Set(current);
              if (value) next.add(key);
              else next.delete(key);
              return next;
            })}
            onDone={checked => void toggleDone(row, checked)}
            onSave={edit => saveRow(row, edit)} />;
        })}
      </ul>}
      {(page > 0 || result?.hasMore) && <div className="flex justify-end gap-2">
        <Button variant="outline" size="sm" disabled={page === 0 || loading || saving} onClick={() => changePage(page - 1)}>Previous</Button>
        <Button variant="outline" size="sm" disabled={!result?.hasMore || loading || saving || !!error} onClick={() => changePage(page + 1)}>Next</Button>
      </div>}
    </div>
    {review?.kind === "planner" && <PlannerExportPreview open actionGroups={review.groups}
      planId={review.destination.planId ?? ""} planName={review.destination.planName}
      defaultBucketId={review.destination.bucketId ?? ""} defaultBucketName={review.destination.bucketName}
      onOpenChange={open => { if (!open) setReview(null); }} onReport={reportExport} />}
    {review?.kind === "todo" && <ToDoExportPreview open actionGroups={review.groups}
      listId={review.destination.todoListId ?? ""} listName={review.destination.todoListName}
      onOpenChange={open => { if (!open) setReview(null); }} onReport={reportExport} />}
  </PageSection>;
}

function ActionListRow({ row, selected, disabled, onSelect, onDone, onSave }: {
  row: OpenActionRow;
  selected: boolean;
  disabled: boolean;
  onSelect: (checked: boolean) => void;
  onDone: (checked: boolean) => void;
  onSave: (edit: ReviewedActionEdit) => Promise<boolean>;
}) {
  const { action, meetingId, meetingTitle, createdAt } = row;
  const [draft, setDraft] = useState<{ owner: string; dueDate: string; original: OpenActionRow["action"] } | null>(null);
  const owner = draft?.owner.trim() || null;
  const dueDate = draft?.dueDate || null;
  const ownerChanged = draft && owner !== draft.original.owner;
  const dueDateChanged = draft && dueDate !== draft.original.dueDate;

  return <li className="space-y-3 py-3">
    <div className="flex flex-wrap items-start gap-3">
      {!action.done && <Checkbox checked={selected} disabled={disabled} className="mt-1"
        aria-label={`Select ${action.title} for export`} onCheckedChange={checked => onSelect(checked === true)} />}
      <div className="min-w-0 flex-1">
        <p className="break-words font-medium">{action.title}</p>
        <Link href={meetingActionUrl(meetingId, action.id)}
          className="break-words rounded-sm text-sm text-muted-foreground underline underline-offset-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
          {meetingTitle} · {formatArchiveDate(createdAt)}
        </Link>
        {!draft && <p className="mt-1 break-words text-sm text-muted-foreground">
          {action.owner || "Unassigned"} · {action.dueDate ? `Due ${action.dueDate}` : "No due date"}
        </p>}
      </div>
      <label className="flex h-8 items-center gap-2 text-sm">
        <Checkbox checked={action.done} disabled={disabled} aria-label={`Done: ${action.title}`}
          onCheckedChange={checked => onDone(checked === true)} />Done
      </label>
      {!draft && <Button variant="outline" size="sm" disabled={disabled} aria-label={`Edit owner and due date for ${action.title}`}
        onClick={() => setDraft({ owner: action.owner ?? "", dueDate: action.dueDate ?? "", original: action })}>Edit</Button>}
    </div>
    {draft && <form className="flex flex-wrap items-end gap-3" onSubmit={async event => {
      event.preventDefault();
      if (disabled || (!ownerChanged && !dueDateChanged)) return;
      const edit: ReviewedActionEdit = { id: action.id };
      if (ownerChanged) edit.owner = { from: draft.original.owner, to: owner };
      if (dueDateChanged) edit.dueDate = { from: draft.original.dueDate, to: dueDate };
      if (await onSave(edit)) setDraft(null);
    }}>
      <label className="min-w-36 flex-1 space-y-1 text-sm">
        <span>Owner</span>
        <Input autoFocus disabled={disabled} maxLength={160} value={draft.owner} onChange={event => {
          const value = event.target.value;
          setDraft(current => current && { ...current, owner: value });
        }} />
      </label>
      <label className="space-y-1 text-sm">
        <span>Due date</span>
        <Input type="date" disabled={disabled} value={draft.dueDate} onChange={event => {
          const value = event.target.value;
          setDraft(current => current && { ...current, dueDate: value });
        }} />
      </label>
      <Button variant="outline" type="button" size="sm" disabled={disabled} onClick={() => setDraft(null)}>Cancel</Button>
      <Button variant="outline" type="submit" size="sm" disabled={disabled || (!ownerChanged && !dueDateChanged)}>Save</Button>
    </form>}
  </li>;
}
