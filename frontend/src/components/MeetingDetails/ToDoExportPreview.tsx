"use client";

import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import { CalendarClock, CheckSquare2, Loader2, User } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
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
  type ExportReport,
} from "@/services/microsoftExportService";

interface Row {
  localId: string;
  title: string;
  details: string | null;
  owner: string | null;
  dueDate: string | null;
  include: boolean;
}

interface ToDoExportPreviewProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  meetingId: string;
  meetingTitle: string;
  listId: string;
  listName?: string;
  getMarkdown: () => Promise<string>;
  onReport: (report: ExportReport) => void;
}

export function ToDoExportPreview({
  open,
  onOpenChange,
  meetingId,
  meetingTitle,
  listId,
  listName,
  getMarkdown,
  onReport,
}: ToDoExportPreviewProps) {
  const [rows, setRows] = useState<Row[]>([]);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    setRows([]);
    (async () => {
      try {
        const md = await getMarkdown();
        const items = await microsoftExportService.previewToDoTasks(
          meetingId,
          meetingTitle,
          md,
        );
        if (cancelled) return;
        setRows(
          items.map((item) => ({
            localId: item.localId,
            title: item.title,
            details: item.details || "",
            owner: item.owner,
            dueDate: item.dueDate,
            include: true,
          })),
        );
      } catch (e) {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [open, meetingId, meetingTitle, getMarkdown]);

  const update = useCallback((index: number, patch: Partial<Row>) => {
    setRows((prev) => prev.map((r, i) => (i === index ? { ...r, ...patch } : r)));
  }, []);

  const selectedCount = rows.filter((r) => r.include && r.title.trim()).length;
  const allSelected = rows.length > 0 && rows.every((r) => r.include);
  const isPreviewReady = !loading && !error && rows.length > 0;
  const canExport = isPreviewReady && !busy && selectedCount > 0;

  const exportTasks = useCallback(async () => {
    const tasks = rows
      .filter((r) => r.include && r.title.trim())
      .map((r) => ({
        localId: r.localId,
        title: r.title.trim(),
        owner: r.owner,
        dueDate: r.dueDate,
        details: r.details?.trim() ? r.details.trim() : null,
      }));
    if (tasks.length === 0) return;
    setBusy(true);
    try {
      const report = await microsoftExportService.exportSelectedToDoTasks(
        meetingId,
        meetingTitle,
        listId,
        tasks,
      );
      onReport(report);
      onOpenChange(false);
    } catch (e) {
      toast.error("Microsoft To Do export failed", {
        description: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setBusy(false);
    }
  }, [rows, meetingId, meetingTitle, listId, onReport, onOpenChange]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <CheckSquare2 className="h-5 w-5 text-primary" />
            Review Microsoft To Do tasks
          </DialogTitle>
          <DialogDescription>
            Pick personal action items to create in{" "}
            <span className="font-medium text-foreground">{listName ?? "your To Do list"}</span>.
            Nothing is created until you export.
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
              <span>{selectedCount} of {rows.length} selected</span>
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
                      <label className="block space-y-1.5">
                        <span className="text-xs font-medium text-muted-foreground">
                          To Do task notes
                        </span>
                        <Textarea
                          value={row.details ?? ""}
                          onChange={(e) => update(index, { details: e.target.value })}
                          disabled={!row.include}
                          placeholder="Add context for the To Do task"
                          className="min-h-[5.5rem] resize-y bg-background text-xs leading-5"
                        />
                      </label>
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
          <Button type="button" onClick={exportTasks} disabled={!canExport}>
            {busy ? (
              <>
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                Exporting…
              </>
            ) : loading ? (
              "Loading tasks…"
            ) : (
              `Export ${selectedCount} task${selectedCount === 1 ? "" : "s"}`
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
