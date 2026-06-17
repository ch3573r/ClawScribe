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
import { PlannerExportPreview } from "./PlannerExportPreview";

interface MeetingExportButtonsProps {
  meetingId: string;
  meetingTitle: string;
  /** Resolves the current summary as markdown. */
  getMarkdown: () => Promise<string>;
}

type Busy = "onenote" | "planner" | null;

// OneNote section names reject ? * \ / : < > | & # ' % ~ " and must be < 50
// chars (Graph 20153 / 20155). The backend sanitizes too, but we keep the
// prefilled value valid and show the user what will actually be created.
const ONENOTE_SECTION_MAX = 49;

function sanitizeSectionName(raw: string): string {
  return raw
    .replace(/[?*\\/:<>|&#'%~"]/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, ONENOTE_SECTION_MAX)
    .trim();
}

/** Default section name: `YYYY-MM-DD <meeting title>` (sanitized, truncated). */
function defaultSectionName(title: string): string {
  const date = new Date().toISOString().slice(0, 10);
  const clean = (title || "Untitled meeting").trim();
  return sanitizeSectionName(`${date} ${clean}`);
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
  const [plannerOpen, setPlannerOpen] = useState(false);
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
    (
      label: string,
      report: {
        overall: string;
        items: Array<{ webUrl: string | null; code: string | null; status: string }>;
      },
    ) => {
      const webUrl = report.items.find((i) => i.webUrl)?.webUrl ?? null;
      // The backend serializes ExportStatus via Debug-lowercase, so success is
      // "succeeded" (not "success").
      if (report.overall === "succeeded") {
        toast.success(`${label} export complete`, {
          description: webUrl ? "Open in Microsoft 365" : undefined,
          action: webUrl
            ? { label: "Open", onClick: () => window.open(webUrl, "_blank") }
            : undefined,
        });
      } else {
        const failing = report.items.find((i) => i.code) ?? report.items[0];
        const reason = failing?.code ?? report.overall;
        toast.warning(`${label} export finished with issues`, {
          description: `Reason: ${reason}`,
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
    const name = sanitizeSectionName(sectionName);
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

  // Planner export opens a review dialog (pick/edit/route action items) rather
  // than creating tasks immediately.
  const openPlanner = useCallback(() => {
    if (!hasPlannerDestination(getExportDestinations())) {
      toast.info("Pick a Planner plan and default bucket first", {
        description: "Settings → Add-ons → Planner task export.",
      });
      return;
    }
    setPlannerOpen(true);
  }, []);

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
          onClick={openPlanner}
          disabled={busy !== null}
          title="Review and export action items to Planner"
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
            <p className="text-xs text-muted-foreground">
              Created as: <span className="font-medium">{sanitizeSectionName(sectionName) || "—"}</span>
              {" · "}
              {sanitizeSectionName(sectionName).length}/{ONENOTE_SECTION_MAX}.
              OneNote disallows {"? * \\ / : < > | & # ' % ~"} and names ≥ 50 chars.
            </p>
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

      <PlannerExportPreview
        open={plannerOpen}
        onOpenChange={(o) => !busy && setPlannerOpen(o)}
        meetingId={meetingId}
        meetingTitle={meetingTitle}
        planId={getExportDestinations().planId ?? ""}
        planName={getExportDestinations().planName}
        defaultBucketId={getExportDestinations().bucketId ?? ""}
        defaultBucketName={getExportDestinations().bucketName}
        getMarkdown={getMarkdown}
        onReport={(report) => reportToast("Planner", report)}
      />
    </div>
  );
}
