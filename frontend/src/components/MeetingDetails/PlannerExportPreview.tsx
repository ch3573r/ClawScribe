"use client";

import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import { ListTodo, Loader2, User, CalendarClock } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  microsoftExportService,
  type BucketInfo,
  type ExportReport,
} from "@/services/microsoftExportService";
import { getExportDestinations } from "@/lib/exportDestinations";
import { useConfig } from "@/contexts/ConfigContext";

interface Row {
  localId: string;
  title: string;
  details: string | null;
  owner: string | null;
  dueDate: string | null;
  bucketId: string;
  include: boolean;
}

interface PlannerExportPreviewProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  meetingId: string;
  meetingTitle: string;
  planId: string;
  planName?: string;
  defaultBucketId: string;
  defaultBucketName?: string;
  getMarkdown: () => Promise<string>;
  onReport: (report: ExportReport) => void;
}

/**
 * Review-and-export dialog for Planner. The user sees every action item parsed
 * from the summary, edits titles inline, deselects ones they don't want, and
 * routes each to a bucket (defaulting to the one chosen in Settings) before any
 * task is created. Nothing reaches Planner until "Export" is pressed.
 */
export function PlannerExportPreview({
  open,
  onOpenChange,
  meetingId,
  meetingTitle,
  planId,
  planName,
  defaultBucketId,
  defaultBucketName,
  getMarkdown,
  onReport,
}: PlannerExportPreviewProps) {
  const { modelConfig } = useConfig();
  const [rows, setRows] = useState<Row[]>([]);
  const [buckets, setBuckets] = useState<BucketInfo[]>([]);
  const [loading, setLoading] = useState(false);
  const [polishing, setPolishing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    (async () => {
      try {
        const [items, bucketList] = await Promise.all([
          getMarkdown().then((md) =>
            microsoftExportService.previewPlannerTasks(meetingId, meetingTitle, md),
          ),
          microsoftExportService.listBuckets(planId),
        ]);
        if (cancelled) return;
        setBuckets(bucketList);
        // Default each task to the bucket chosen in Settings (fall back to first).
        const fallbackBucket =
          bucketList.find((b) => b.id === defaultBucketId)?.id ??
          bucketList[0]?.id ??
          defaultBucketId;
        const baseRows: Row[] = items.map((item) => ({
          localId: item.localId,
          title: item.title,
          details: null,
          owner: item.owner,
          dueDate: item.dueDate,
          bucketId: fallbackBucket,
          include: true,
        }));
        setRows(baseRows);

        // Optional AI polish (Settings → Add-ons → Planner). Reviewed below, so a
        // poor rewrite never lands silently; on failure we keep the raw titles.
        // All providers (including Codex, via its app-server) are supported.
        const aiPolish = getExportDestinations().plannerAiPolish ?? false;
        if (aiPolish && baseRows.length > 0) {
          setPolishing(true);
          try {
            const polished = await microsoftExportService.polishPlannerTasks(
              modelConfig.provider,
              modelConfig.model,
              baseRows.map((r) => ({ title: r.title, owner: r.owner, dueDate: r.dueDate })),
            );
            if (!cancelled && polished.length === baseRows.length) {
              setRows((prev) =>
                prev.map((r, i) => ({
                  ...r,
                  title: polished[i].title || r.title,
                  details: polished[i].details || null,
                })),
              );
            }
          } catch (e) {
            if (!cancelled) {
              toast.info("Couldn't AI-polish tasks — using the original titles.", {
                description: e instanceof Error ? e.message : String(e),
              });
            }
          } finally {
            if (!cancelled) setPolishing(false);
          }
        }
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [
    open,
    meetingId,
    meetingTitle,
    planId,
    defaultBucketId,
    getMarkdown,
    modelConfig.provider,
    modelConfig.model,
  ]);

  const update = useCallback((index: number, patch: Partial<Row>) => {
    setRows((prev) => prev.map((r, i) => (i === index ? { ...r, ...patch } : r)));
  }, []);

  const selectedCount = rows.filter((r) => r.include && r.title.trim()).length;
  const allSelected = rows.length > 0 && rows.every((r) => r.include);

  const exportTasks = useCallback(async () => {
    const tasks = rows
      .filter((r) => r.include && r.title.trim())
      .map((r) => ({
        localId: r.localId,
        title: r.title.trim(),
        owner: r.owner,
        dueDate: r.dueDate,
        bucketId: r.bucketId,
        details: r.details,
      }));
    if (tasks.length === 0) return;
    setBusy(true);
    try {
      const report = await microsoftExportService.exportSelectedPlannerTasks(
        meetingId,
        meetingTitle,
        planId,
        tasks,
      );
      onReport(report);
      onOpenChange(false);
    } catch (e) {
      toast.error("Planner export failed", {
        description: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setBusy(false);
    }
  }, [rows, meetingId, meetingTitle, planId, onReport, onOpenChange]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <ListTodo className="h-5 w-5 text-primary" />
            Review Planner tasks
          </DialogTitle>
          <DialogDescription>
            Pick which action items to create in{" "}
            <span className="font-medium text-foreground">{planName ?? "your plan"}</span>,
            edit titles, and choose a bucket for each. Nothing is created until you export.
          </DialogDescription>
        </DialogHeader>

        {loading ? (
          <div className="flex items-center justify-center gap-2 py-12 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
            Reading action items…
          </div>
        ) : error ? (
          <div className="rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
            {error}
          </div>
        ) : rows.length === 0 ? (
          <div className="rounded-lg border border-border bg-muted p-6 text-center text-sm text-muted-foreground">
            No action items were found in this summary. Generate or edit the summary so it
            includes an &quot;Action items&quot; section, then try again.
          </div>
        ) : (
          <>
            <div className="flex items-center justify-between px-1 text-xs text-muted-foreground">
              <button
                type="button"
                className="font-medium hover:text-foreground"
                onClick={() => {
                  const next = !allSelected;
                  setRows((prev) => prev.map((r) => ({ ...r, include: next })));
                }}
              >
                {allSelected ? "Deselect all" : "Select all"}
              </button>
              <span className="flex items-center gap-2">
                {polishing && (
                  <span className="flex items-center gap-1 text-primary">
                    <Loader2 className="h-3 w-3 animate-spin" />
                    Polishing with AI…
                  </span>
                )}
                {selectedCount} of {rows.length} selected
              </span>
            </div>

            <div className="max-h-[48vh] space-y-2 overflow-y-auto pr-1">
              {rows.map((row, index) => (
                <div
                  key={row.localId || index}
                  className={`rounded-lg border p-3 transition ${
                    row.include
                      ? "border-primary/40 bg-primary/5"
                      : "border-border bg-background opacity-60"
                  }`}
                >
                  <div className="flex items-start gap-3">
                    <input
                      type="checkbox"
                      checked={row.include}
                      onChange={(e) => update(index, { include: e.target.checked })}
                      aria-label={`Include "${row.title}"`}
                      className="mt-1.5 h-4 w-4 shrink-0 accent-primary"
                    />
                    <div className="min-w-0 flex-1 space-y-2">
                      <input
                        type="text"
                        value={row.title}
                        onChange={(e) => update(index, { title: e.target.value })}
                        disabled={!row.include}
                        className="w-full rounded-md border border-border bg-background px-2.5 py-1.5 text-sm font-medium text-foreground focus:border-primary focus:outline-none focus:ring-1 focus:ring-primary disabled:cursor-not-allowed"
                      />
                      {row.details && (
                        <p className="text-xs leading-5 text-muted-foreground">{row.details}</p>
                      )}
                      <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
                        {row.owner && (
                          <span className="inline-flex items-center gap-1">
                            <User className="h-3 w-3" />
                            {row.owner}
                          </span>
                        )}
                        {row.dueDate && (
                          <span className="inline-flex items-center gap-1">
                            <CalendarClock className="h-3 w-3" />
                            {row.dueDate}
                          </span>
                        )}
                        <label className="ml-auto inline-flex items-center gap-1.5">
                          <span>Bucket</span>
                          <select
                            value={row.bucketId}
                            onChange={(e) => update(index, { bucketId: e.target.value })}
                            disabled={!row.include}
                            className="rounded-md border border-border bg-muted px-2 py-1 text-xs text-foreground disabled:cursor-not-allowed"
                          >
                            {buckets.length === 0 && (
                              <option value={defaultBucketId}>
                                {defaultBucketName ?? "Default bucket"}
                              </option>
                            )}
                            {buckets.map((b) => (
                              <option key={b.id} value={b.id}>
                                {b.name}
                              </option>
                            ))}
                          </select>
                        </label>
                      </div>
                    </div>
                  </div>
                </div>
              ))}
            </div>
          </>
        )}

        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)} disabled={busy}>
            Cancel
          </Button>
          <Button type="button" onClick={exportTasks} disabled={busy || selectedCount === 0}>
            {busy ? (
              <>
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                Exporting…
              </>
            ) : (
              `Export ${selectedCount} task${selectedCount === 1 ? "" : "s"}`
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
