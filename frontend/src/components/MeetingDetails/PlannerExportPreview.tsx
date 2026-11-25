"use client";

import { useEffect, useRef, useState } from "react";
import { CalendarClock, CheckSquare2, ListTodo, Loader2, User } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { useConfig } from "@/contexts/ConfigContext";
import { getExportDestinations } from "@/lib/exportDestinations";
import { actionRowKey, type ActionExportGroup } from "@/lib/meeting-follow-through";
import { editReviewedActions, savePreviewedSummaryActions, type ReviewedActionEdit, type SavedAction } from "@/services/meetingActionsService";
import {
  microsoftExportService,
  TASK_IDENTITY_REVIEW_MESSAGE,
  taskIdentityReviewIds,
  taskPreviewCandidates,
  type BucketInfo,
  type ExportReport,
  type PlannerTaskPreview,
  type TaskPreviewCandidate,
} from "@/services/microsoftExportService";

const EMPTY_BUCKET = "__unset_bucket__";

type PreviewItem = PlannerTaskPreview & { savedDetails?: string | null };

interface TaskRow {
  item: PreviewItem;
  title: string;
  details: string;
  bucketId: string;
  include: boolean;
}

interface TaskGroup {
  meetingId: string;
  meetingTitle: string;
  markdown: string;
  rows: TaskRow[];
  candidates: readonly TaskPreviewCandidate[];
  suggestionCount: number;
}

type PreviewSource =
  | { meetingId: string; meetingTitle: string; getMarkdown: () => Promise<string>; actionGroups?: never }
  | { actionGroups: readonly ActionExportGroup[]; meetingId?: never; meetingTitle?: never; getMarkdown?: never };

export type TaskExportPreviewProps = PreviewSource & {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onReport: (report: ExportReport) => void;
  onReviewActions?: () => void;
};

type TaskDestination =
  | { kind: "planner"; id: string; name?: string; bucketId: string; bucketName?: string }
  | { kind: "todo"; id: string; name?: string };

type PlannerExportPreviewProps = TaskExportPreviewProps & {
  planId: string;
  planName?: string;
  defaultBucketId: string;
  defaultBucketName?: string;
};

export function PlannerExportPreview(props: PlannerExportPreviewProps) {
  return <TaskExportPreview {...props} destination={{
    kind: "planner", id: props.planId, name: props.planName,
    bucketId: props.defaultBucketId, bucketName: props.defaultBucketName,
  }} />;
}

function rowChanged(row: TaskRow) {
  return row.title.trim() !== row.item.title || row.details.trim() !== (row.item.details || "").trim();
}

function savedRow(row: TaskRow, action: SavedAction): TaskRow {
  const details = action.details ?? row.details;
  return {
    ...row,
    title: action.title,
    details,
    item: {
      ...row.item, localId: action.id, originalTitleHash: null, title: action.title,
      details, savedDetails: action.details ?? null, owner: action.owner, dueDate: action.dueDate,
    },
  };
}

function savedEdit({ item, title, details }: TaskRow): ReviewedActionEdit {
  const edit: ReviewedActionEdit = { id: item.localId };
  if (title.trim() !== item.title) edit.title = { from: item.title, to: title.trim() };
  if (details.trim() !== (item.details || "").trim()) {
    edit.details = { from: item.savedDetails ?? null, to: details.trim() };
  }
  return edit;
}

async function saveGroup(group: TaskGroup): Promise<TaskGroup> {
  const changed = group.rows.filter(rowChanged);
  if (!changed.length) return group;
  const isSummary = group.rows.some(row => row.item.originalTitleHash);
  const actions = isSummary
    ? await savePreviewedSummaryActions(group.meetingId, group.markdown, changed.flatMap(row => {
      const { localId, originalTitleHash } = row.item;
      if (!originalTitleHash) return [];
      const edit = savedEdit(row);
      return [{ localId, originalTitleHash, title: edit.title?.to, details: edit.details?.to ?? undefined }];
    }))
    : await editReviewedActions(group.meetingId, changed.map(savedEdit));
  const byId = new Map(actions.map(action => [isSummary ? action.legacyAction?.id : action.id, action]));
  return {
    ...group,
    candidates: isSummary ? [] : group.candidates,
    rows: group.rows.map(row => {
      const saved = byId.get(row.item.localId);
      return saved ? savedRow(row, saved) : row;
    }),
  };
}

export function TaskExportPreview({
  open,
  onOpenChange,
  meetingId,
  meetingTitle,
  getMarkdown,
  actionGroups,
  destination,
  onReport,
  onReviewActions,
}: TaskExportPreviewProps & { destination: TaskDestination }) {
  const { modelConfig } = useConfig();
  const [groups, setGroups] = useState<TaskGroup[]>([]);
  const [buckets, setBuckets] = useState<BucketInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [operation, setOperation] = useState<"polishing" | "saving" | "exporting" | null>(null);
  const [reviewIds, setReviewIds] = useState(() => new Set<string>());
  const lifecycle = useRef(0);
  const pending = useRef<symbol | null>(null);
  const planner = destination.kind === "planner";
  const destinationId = destination.id;
  const defaultBucket = destination.kind === "planner" ? destination.bucketId : "";
  const label = planner ? "Planner" : "Microsoft To Do";
  const Icon = planner ? ListTodo : CheckSquare2;

  useEffect(() => {
    if (!open) return;
    const generation = ++lifecycle.current;
    setLoading(true);
    setError("");
    setGroups([]);
    setBuckets([]);
    setReviewIds(new Set());
    setOperation(null);
    pending.current = null;
    void (async () => {
      try {
        const source = actionGroups ?? [{ meetingId: meetingId!, meetingTitle: meetingTitle! }];
        const [previews, bucketList] = await Promise.all([
          Promise.all(source.map(async group => {
            const markdown = getMarkdown ? await getMarkdown() : "";
            const preview = await (planner ? microsoftExportService.previewPlannerTasks(
              group.meetingId, group.meetingTitle, markdown,
            ) : microsoftExportService.previewToDoTasks(group.meetingId, group.meetingTitle, markdown));
            const ids = "actionIds" in group ? new Set(group.actionIds) : null;
            const items = ids ? preview.items.filter(item => ids.has(item.localId) && !item.originalTitleHash) : preview.items;
            if (ids && items.length !== ids.size) throw new Error("Selected actions changed. Close this preview and review the latest actions.");
            return { group, markdown, preview, items };
          })),
          planner ? microsoftExportService.listBuckets(destinationId) : Promise.resolve([]),
        ]);
        if (generation !== lifecycle.current) return;
        const bucketId = bucketList.find(bucket => bucket.id === defaultBucket)?.id ?? bucketList[0]?.id ?? defaultBucket;
        setBuckets(bucketList);
        setGroups(previews.map(({ group, markdown, preview, items }) => ({
          meetingId: group.meetingId, meetingTitle: group.meetingTitle, markdown,
          candidates: preview.candidates ?? taskPreviewCandidates(preview.items),
          suggestionCount: preview.suggestionCount,
          rows: items.map(item => ({ item, title: item.title, details: item.details || "", include: true, bucketId })),
        })));
      } catch (failure) {
        if (generation === lifecycle.current) setError(String(failure instanceof Error ? failure.message : failure));
      } finally {
        if (generation === lifecycle.current) setLoading(false);
      }
    })();
    return () => { lifecycle.current++; };
  }, [open, meetingId, meetingTitle, getMarkdown, actionGroups, planner, destinationId, defaultBucket]);

  const rows = groups.flatMap(group => group.rows.map(row => ({ group, row })));
  const selected = rows.filter(({ row }) => row.include);
  const allSelected = rows.length > 0 && selected.length === rows.length;
  const canExport = !loading && !operation && !error && selected.length > 0
    && selected.every(({ row }) => row.title.trim() && (!planner || row.bucketId));
  const changed = rows.filter(({ row }) => rowChanged(row));
  const canSave = !loading && !operation && changed.length > 0 && changed.every(({ row }) => row.title.trim());
  const summaryDraft = rows.some(({ row }) => row.item.originalTitleHash);
  const suggestionCount = actionGroups ? 0 : groups[0]?.suggestionCount ?? 0;
  const savingOrExporting = operation === "saving" || operation === "exporting";

  function update(groupId: string, localId: string, patch: Partial<TaskRow>) {
    setGroups(current => current.map(group => group.meetingId !== groupId ? group : {
      ...group, rows: group.rows.map(row => row.item.localId === localId ? { ...row, ...patch } : row),
    }));
  }

  function reviewActions() {
    onOpenChange(false);
    onReviewActions?.();
  }

  async function run(kind: NonNullable<typeof operation>, work: (current: () => boolean) => Promise<void>) {
    if (pending.current) return;
    const token = Symbol();
    const generation = lifecycle.current;
    const current = () => generation === lifecycle.current;
    pending.current = token;
    setOperation(kind);
    try {
      await work(current);
    } finally {
      if (pending.current === token) pending.current = null;
      if (current()) setOperation(null);
    }
  }

  function polishSelected() {
    if (!canExport) return;
    void run("polishing", async current => {
      try {
        const polished = await microsoftExportService.polishPlannerTasks(
          modelConfig.provider, modelConfig.model,
          selected.map(({ row }) => ({ title: row.title, owner: row.item.owner, dueDate: row.item.dueDate })),
        );
        if (!current()) return;
        if (polished.length !== selected.length) throw new Error("Incomplete polish result.");
        selected.forEach(({ group, row }, index) => {
          const replacement = polished[index];
          update(group.meetingId, row.item.localId, {
            title: replacement.title?.trim() || row.title,
            details: replacement.details?.trim() || row.details,
          });
        });
      } catch {
        if (current()) toast.info("Could not polish tasks. Your draft is still available.");
      }
    });
  }

  function saveEdits() {
    if (!canSave) return;
    void run("saving", async current => {
      let savedCount = 0;
      for (const group of groups) {
        try {
          const saved = await saveGroup(group);
          if (!current()) return;
          if (saved === group) continue;
          savedCount++;
          setGroups(previous => previous.map(value => value.meetingId === group.meetingId ? saved : value));
          setReviewIds(previous => {
            const next = new Set(previous);
            group.rows.forEach((row, index) => {
              const oldKey = actionRowKey(group.meetingId, row.item.localId);
              if (next.delete(oldKey)) next.add(actionRowKey(group.meetingId, saved.rows[index].item.localId));
            });
            return next;
          });
        } catch (failure) {
          if (current()) toast.error("Could not save action edits", {
            description: `${group.meetingTitle}. ${String(failure)}${savedCount ? ` ${savedCount} meeting(s) saved before the error.` : ""}`,
          });
          return;
        }
      }
      if (current()) toast.success("Action edits saved to meetings.");
    });
  }

  function exportTasks() {
    if (!canExport) return;
    void run("exporting", async current => {
      const needsReview = new Set<string>();
      const requests = groups.flatMap(group => {
        const tasks = group.rows.filter(row => row.include).map(row => ({
          localId: row.item.localId, originalTitleHash: row.item.originalTitleHash, title: row.title.trim(),
          owner: row.item.owner, dueDate: row.item.dueDate, details: row.details.trim() || null, bucketId: row.bucketId,
        }));
        return tasks.length ? [{ group, tasks }] : [];
      });
      let processed = 0;
      const reports: ExportReport[] = [];
      try {
        for (const { group, tasks } of requests) {
          const report = await (planner
            ? microsoftExportService.exportSelectedPlannerTasks(group.meetingId, group.meetingTitle, destinationId, tasks, group.candidates)
            : microsoftExportService.exportSelectedToDoTasks(
              group.meetingId, group.meetingTitle, destinationId,
              tasks.map(({ bucketId, ...task }) => task),
              group.candidates,
            ));
          if (!current()) return;
          reports.push(report);
          processed += tasks.length;
          for (const id of taskIdentityReviewIds(report)) needsReview.add(actionRowKey(group.meetingId, id));
          if (report.overall !== "succeeded") break;
        }
      } catch (failure) {
        if (current()) toast.error(`${label} export failed`, { description: String(failure) });
      }
      if (!current()) return;
      setReviewIds(needsReview);
      if (reports.length) {
        const last = reports[reports.length - 1];
        onReport({ ...last, items: reports.flatMap(report => report.items) });
      }
      if (processed < selected.length && actionGroups) {
        toast.warning(`${selected.length - processed} selected tasks were not attempted.`);
      }
      if (processed === selected.length && needsReview.size === 0) onOpenChange(false);
    });
  }

  return <Dialog open={open} onOpenChange={value => { if (!savingOrExporting) onOpenChange(value); }}>
    <DialogContent className="max-h-[90vh] max-w-2xl overflow-y-auto">
      <DialogHeader>
        <DialogTitle className="flex items-center gap-2"><Icon className="h-5 w-5 text-primary" />Review {label} tasks</DialogTitle>
        <DialogDescription>
          Review tasks for <span className="font-medium text-foreground">{destination.name ?? (planner ? "your Planner plan" : "your To Do list")}</span>.
          {" "}Use Save edits to keep changes in your meetings.
        </DialogDescription>
      </DialogHeader>

      {loading ? <p role="status" className="flex items-center justify-center gap-2 py-10 text-sm text-muted-foreground">
        <Loader2 className="h-4 w-4 animate-spin" />Reading action items…
      </p> : error ? <p role="alert" className="rounded-lg border border-destructive/30 p-3 text-sm text-destructive">{error}</p>
        : <>
          {suggestionCount > 0 && onReviewActions && <div className="flex flex-wrap items-center justify-between gap-2 text-sm">
            <span>{suggestionCount} new summary {suggestionCount === 1 ? "suggestion" : "suggestions"}</span>
            <Button variant="outline" size="sm" disabled={!!operation} onClick={reviewActions}>Review suggestions</Button>
          </div>}
          {!rows.length && <div className="space-y-2 text-sm text-muted-foreground">
            <p>No open actions found.</p>
            {onReviewActions && <Button variant="outline" onClick={reviewActions}>Review meeting actions</Button>}
          </div>}
          {reviewIds.size > 0 && <p role="status" className="text-sm text-warning">{TASK_IDENTITY_REVIEW_MESSAGE}</p>}
          {rows.length > 0 && <>
            <div className="flex flex-wrap items-center gap-2">
              <Button variant="ghost" size="sm" disabled={!!operation} onClick={() => {
                setGroups(previous => previous.map(group => ({ ...group, rows: group.rows.map(row => ({ ...row, include: !allSelected })) })));
              }}>{allSelected ? "Deselect all" : "Select all"}</Button>
              {getExportDestinations().plannerAiPolish && <Button type="button" size="sm" variant="outline" disabled={!canExport}
                onClick={polishSelected}>Polish selected with AI</Button>}
              <span className="ml-auto text-xs text-muted-foreground">{selected.length} of {rows.length} selected</span>
            </div>
            {operation === "polishing" && <p role="status" className="text-sm text-muted-foreground">Polishing with AI…</p>}
            {summaryDraft && changed.length > 0 && <p className="text-sm text-muted-foreground">Saving edits also saves the previewed summary actions to the meeting.</p>}
            <ul className="space-y-2">
              {rows.map(({ group, row }) => {
                const { localId, owner, dueDate } = row.item;
                const id = actionRowKey(group.meetingId, localId);
                const edit = (patch: Partial<TaskRow>) => update(group.meetingId, localId, patch);
                const disabled = !row.include || !!operation;
                return <li key={id} className="flex items-start gap-3 rounded-lg border border-border p-3">
                  <Checkbox aria-label={`Include ${row.title}`} checked={row.include} disabled={!!operation}
                    onCheckedChange={checked => edit({ include: checked === true })} className="mt-2" />
                  <div className="min-w-0 flex-1 space-y-2">
                    {actionGroups && <p className="break-words text-xs text-muted-foreground">{group.meetingTitle}</p>}
                    <Input aria-label="Task title" value={row.title} maxLength={255} disabled={disabled} onChange={event => edit({ title: event.target.value })} />
                    {reviewIds.has(id) && <p className="text-sm text-warning">This task may already exist in the destination.</p>}
                    <label className="block space-y-1 text-xs text-muted-foreground">
                      <span>Task notes</span>
                      <Textarea value={row.details} maxLength={4000} disabled={disabled} onChange={event => edit({ details: event.target.value })}
                        className="min-h-20 resize-y text-sm" />
                    </label>
                    <div className="flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
                      {owner && <span className="inline-flex items-center gap-1"><User className="h-3 w-3" />{owner}</span>}
                      {dueDate && <span className="inline-flex items-center gap-1"><CalendarClock className="h-3 w-3" />{dueDate}</span>}
                      {destination.kind === "planner" && <Select value={row.bucketId || EMPTY_BUCKET} disabled={disabled}
                        onValueChange={value => edit({ bucketId: value === EMPTY_BUCKET ? "" : value })}>
                        <SelectTrigger aria-label="Bucket" className="ml-auto h-8 w-auto"><SelectValue /></SelectTrigger>
                        <SelectContent>
                          {buckets.length ? buckets.map(bucket => <SelectItem key={bucket.id} value={bucket.id}>{bucket.name}</SelectItem>)
                            : <SelectItem value={defaultBucket || EMPTY_BUCKET}>{destination.bucketName ?? "Default bucket"}</SelectItem>}
                        </SelectContent>
                      </Select>}
                    </div>
                  </div>
                </li>;
              })}
            </ul>
          </>}
        </>}
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)} disabled={savingOrExporting}>Cancel</Button>
        {changed.length > 0 && <Button variant="outline" onClick={saveEdits} disabled={!canSave}>
          {operation === "saving" ? "Saving edits…" : "Save edits"}
        </Button>}
        <Button onClick={exportTasks} disabled={!canExport}>
          {operation === "exporting" ? "Exporting…" : `Export ${selected.length} ${selected.length === 1 ? "task" : "tasks"}`}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>;
}
