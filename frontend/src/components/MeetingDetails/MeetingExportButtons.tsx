"use client";

import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import { NotebookTabs, ListTodo, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
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
  type MicrosoftConnectionInfo,
} from "@/services/microsoftExportService";
import {
  getExportDestinations,
  hasOneNoteDestination,
  hasPlannerDestination,
} from "@/lib/exportDestinations";

interface MeetingExportButtonsProps {
  meetingId: string;
  meetingTitle: string;
  /** Resolves the current summary as markdown. */
  getMarkdown: () => Promise<string>;
}

type Busy = "onenote" | "planner" | null;

/** Default section name: `YYYY-MM-DD: <meeting title>`. */
function defaultSectionName(title: string): string {
  const date = new Date().toISOString().slice(0, 10);
  const clean = (title || "Untitled meeting").trim();
  return `${date}: ${clean}`;
}

/**
 * Per-meeting export actions shown in the summary view. OneNote is always
 * available once Microsoft is connected and a notebook is chosen; exporting
 * opens a dialog to name the section that will be created (a dated section per
 * export — this avoids the OneNote 5,000-item enumeration limit). Planner
 * appears only when the summary has action items.
 */
export function MeetingExportButtons({
  meetingId,
  meetingTitle,
  getMarkdown,
}: MeetingExportButtonsProps) {
  const [connected, setConnected] = useState(false);
  const [hasActionItems, setHasActionItems] = useState(false);
  const [busy, setBusy] = useState<Busy>(null);

  const [oneNoteOpen, setOneNoteOpen] = useState(false);
  const [sectionName, setSectionName] = useState("");

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const status: MicrosoftConnectionInfo =
          await microsoftExportService.connectionStatus();
        if (!cancelled) setConnected(status.state === "connected");
      } catch {
        if (!cancelled) setConnected(false);
      }
      try {
        const md = await getMarkdown();
        const present = md.trim()
          ? await microsoftExportService.summaryHasActionItems(md)
          : false;
        if (!cancelled) setHasActionItems(present);
      } catch {
        if (!cancelled) setHasActionItems(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [meetingId, getMarkdown]);

  const reportToast = useCallback(
    (label: string, report: { overall: string; items: Array<{ webUrl: string | null }> }) => {
      const webUrl = report.items.find((i) => i.webUrl)?.webUrl ?? null;
      if (report.overall === "success" || report.overall === "skipped") {
        toast.success(`${label} export complete`, {
          description: webUrl ? "Open in Microsoft 365" : undefined,
          action: webUrl
            ? { label: "Open", onClick: () => window.open(webUrl, "_blank") }
            : undefined,
        });
      } else {
        toast.warning(`${label} export finished with issues`, {
          description: `Status: ${report.overall}`,
        });
      }
    },
    [],
  );

  // Opening the OneNote dialog: require a notebook, then prefill the section name.
  const openOneNote = useCallback(() => {
    if (!hasOneNoteDestination(getExportDestinations())) {
      toast.info("Pick a OneNote notebook first", {
        description: "Settings → Add-ons → OneNote export.",
      });
      return;
    }
    setSectionName(defaultSectionName(meetingTitle));
    setOneNoteOpen(true);
  }, [meetingTitle]);

  const confirmOneNote = useCallback(async () => {
    const name = sectionName.trim();
    if (!name) {
      toast.info("Enter a section name.");
      return;
    }
    const { notebookId } = getExportDestinations();
    setBusy("onenote");
    try {
      const md = await getMarkdown();
      if (!md.trim()) {
        toast.info("Nothing to export yet — generate a summary first.");
        return;
      }
      const report = await microsoftExportService.exportMeetingToOneNoteSection(
        meetingId,
        meetingTitle,
        md,
        notebookId!,
        name,
      );
      reportToast("OneNote", report);
      setOneNoteOpen(false);
    } catch (e) {
      toast.error("OneNote export failed", {
        description: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setBusy(null);
    }
  }, [sectionName, getMarkdown, meetingId, meetingTitle, reportToast]);

  const exportPlanner = useCallback(async () => {
    if (!hasPlannerDestination(getExportDestinations())) {
      toast.info("Pick a Planner plan and bucket first", {
        description: "Settings → Add-ons → Planner task export.",
      });
      return;
    }
    setBusy("planner");
    try {
      const md = await getMarkdown();
      const { planId, bucketId } = getExportDestinations();
      const report = await microsoftExportService.exportMeetingMarkdownToPlanner(
        meetingId,
        meetingTitle,
        md,
        planId!,
        bucketId!,
      );
      reportToast("Planner", report);
    } catch (e) {
      toast.error("Planner export failed", {
        description: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setBusy(null);
    }
  }, [getMarkdown, meetingId, meetingTitle, reportToast]);

  // Export requires a Microsoft connection; the Add-ons panel handles sign-in.
  if (!connected) return null;

  return (
    <div className="flex items-center gap-2">
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={openOneNote}
        disabled={busy !== null}
        title="Export this meeting's summary to a new OneNote section"
      >
        {busy === "onenote" ? (
          <Loader2 className="mr-2 h-4 w-4 animate-spin" />
        ) : (
          <NotebookTabs className="mr-2 h-4 w-4" />
        )}
        OneNote
      </Button>

      {hasActionItems && (
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={exportPlanner}
          disabled={busy !== null}
          title="Export action items to Planner"
        >
          {busy === "planner" ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          ) : (
            <ListTodo className="mr-2 h-4 w-4" />
          )}
          Planner
        </Button>
      )}

      <Dialog open={oneNoteOpen} onOpenChange={(o) => !busy && setOneNoteOpen(o)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Export to OneNote</DialogTitle>
            <DialogDescription>
              A new section with this name will be created in your selected
              notebook ({getExportDestinations().notebookName ?? "notebook"}).
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-2">
            <Label htmlFor="onenote-section-name">Section name</Label>
            <Input
              id="onenote-section-name"
              value={sectionName}
              onChange={(e) => setSectionName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void confirmOneNote();
              }}
              autoFocus
            />
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => setOneNoteOpen(false)}
              disabled={busy === "onenote"}
            >
              Cancel
            </Button>
            <Button type="button" onClick={confirmOneNote} disabled={busy === "onenote"}>
              {busy === "onenote" && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
              Create section &amp; export
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
