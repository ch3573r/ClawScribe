"use client";

import { openExternal } from '@/lib/openExternal';
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { Loader2, Upload, ChevronDown } from "lucide-react";
import {
  ConfluenceIcon,
  WordIcon,
  OneNoteIcon,
  OneDriveIcon,
  PlannerIcon,
  ToDoIcon,
} from "@/components/IntegrationIcons";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
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
  ONENOTE_LARGE_LIBRARY_MESSAGE,
  TASK_IDENTITY_REVIEW_CODE,
  isOneNoteLargeLibraryError,
  microsoftExportService,
  type MicrosoftConnectionInfo,
  type OneNoteResume,
  type MeetingExportHistory,
  type NotebookInfo,
} from "@/services/microsoftExportService";
import {
  buildConfluenceDraftMarkdown,
  markdownToConfluenceHtml,
  writeConfluenceDraftToClipboard,
} from "@/lib/confluenceDraft";
import {
  getExportDestinations,
  hasPlannerDestination,
  hasToDoDestination,
  setExportDestinations,
} from "@/lib/exportDestinations";
import { confluenceExportService } from "@/services/confluenceExportService";
import { PlannerExportPreview } from "./PlannerExportPreview";
import { ToDoExportPreview } from "./ToDoExportPreview";
import { ExportContentOptions } from "./ExportContentOptions";
import { defaultExportOptions, prepareMeetingExport, readExportSummary } from "@/lib/meetingExportContent";
import { safeDocumentName } from "@/lib/library";

interface MeetingExportButtonsProps {
  meetingId: string;
  meetingTitle: string;
  meetingCreatedAt?: string;
  /** Resolves the current summary as markdown. */
  getMarkdown: () => Promise<string>;
  disabled?: boolean;
  onReviewActions?: () => void;
}

type DocumentDestination = "word" | "onedrive" | "confluence";
type Busy = DocumentDestination | "onenote" | null;

const documentLabels: Record<DocumentDestination, { title: string; description: string; submit: string }> = {
  word: { title: "Export Word document", description: "Save an editable .docx on this computer. Microsoft sign-in is not required.", submit: "Save as Word document" },
  onedrive: { title: "Export to OneDrive", description: "Upload the selected content as DOCX and, if enabled in Settings, PDF to your saved OneDrive destination.", submit: "Upload to OneDrive" },
  confluence: { title: "Export to Confluence", description: "Copy the selected content as a browser draft or publish it using your configured Confluence connection.", submit: "Export to Confluence" },
};

const EMPTY_NOTEBOOK_VALUE = "__unset_notebook__";
const ONENOTE_NOTEBOOK_MAX = 128;
function sanitizeNotebookName(raw: string): string {
  return raw
    .replace(/[?*\\/:<>|'#]/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, ONENOTE_NOTEBOOK_MAX)
    .trim();
}

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

function defaultDatedTitle(title: string, createdAt?: string): string {
  const date = createdAt ? new Date(createdAt) : new Date();
  const d = Number.isNaN(date.getTime()) ? new Date() : date;
  const stamp = [
    d.getFullYear(),
    String(d.getMonth() + 1).padStart(2, "0"),
    String(d.getDate()).padStart(2, "0"),
  ].join("-") + " " + [
    String(d.getHours()).padStart(2, "0"),
    String(d.getMinutes()).padStart(2, "0"),
  ].join("-");
  const clean = (title || "Untitled meeting").trim();
  return `${stamp} ${clean}`.slice(0, 240).trim();
}

function defaultConfluencePageTitle(title: string, createdAt?: string): string {
  return defaultDatedTitle(title, createdAt);
}

function historyStatus(status: string): string {
  switch (status) {
    case "succeeded": return "Exported";
    case "pending": return "Awaiting confirmation";
    case "access_denied": return "Access denied";
    case "tenant_blocked": return "Blocked by organization policy";
    case "partial_failure": return "Partially exported";
    case "unknown_after_submit": return "Outcome unknown";
    case "failed_auth": return "Sign-in expired";
    case "destination_not_found": return "Destination unavailable";
    default: return "Failed";
  }
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Document exports share content options and stay available without a summary.
 * Microsoft destinations require a connection; task exports retain their review.
 */
export function MeetingExportButtons({
  meetingId,
  meetingTitle,
  meetingCreatedAt,
  getMarkdown,
  disabled = false,
  onReviewActions,
}: MeetingExportButtonsProps) {
  const [connected, setConnected] = useState(false);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [history, setHistory] = useState<MeetingExportHistory[] | null>(null);
  const [historyError, setHistoryError] = useState("");
  const [reviewItem, setReviewItem] = useState<MeetingExportHistory | null>(null);
  const [reviewOutcome, setReviewOutcome] = useState("");
  const [remoteId, setRemoteId] = useState("");
  const [savingReview, setSavingReview] = useState(false);
  const [reviewError, setReviewError] = useState("");
  const [busy, setBusy] = useState<Busy>(null);
  const pending = useRef(false);
  const [documentDestination, setDocumentDestination] = useState<DocumentDestination | null>(null);
  const [exportOptions, setExportOptions] = useState(() => defaultExportOptions());

  const openDocumentExport = (destination: DocumentDestination) => {
    setExportOptions(defaultExportOptions(destination === "word" ? "both" : "summary"));
    setDocumentDestination(destination);
  };

  const [oneNoteOpen, setOneNoteOpen] = useState(false);
  const [resumes, setResumes] = useState<OneNoteResume[] | null>(null);
  const [resumeError, setResumeError] = useState("");
  const [resumeId, setResumeId] = useState("new");
  const [replaceOneNotePage, setReplaceOneNotePage] = useState(false);
  const [oneNoteDraft, setOneNoteDraft] = useState<string | null>(null);
  const selectedResume = resumes?.find(attempt => attempt.id === resumeId);
  const showNewSection = resumes !== null && !selectedResume;
  const [plannerOpen, setPlannerOpen] = useState(false);
  const [todoOpen, setToDoOpen] = useState(false);
  const [oneNoteNotebooks, setOneNoteNotebooks] = useState<NotebookInfo[]>([]);
  const [oneNoteNotebookId, setOneNoteNotebookId] = useState("");
  const newNotebookInputRef = useRef<HTMLInputElement>(null);
  const focusNewNotebookOnClose = useRef(false);
  const [oneNoteSavedNotebookName, setOneNoteSavedNotebookName] = useState<string | null>(null);
  const [oneNoteSectionName, setOneNoteSectionName] = useState("");
  const [oneNotePageTitle, setOneNotePageTitle] = useState("");
  const [loadingOneNoteNotebooks, setLoadingOneNoteNotebooks] = useState(false);
  const [oneNoteNotebookListingLimited, setOneNoteNotebookListingLimited] = useState(false);
  const [creatingNotebook, setCreatingNotebook] = useState(false);
  const [newNotebookName, setNewNotebookName] = useState("");
  const [savingNotebook, setSavingNotebook] = useState(false);

  const loadOneNoteNotebooks = useCallback(async () => {
    setLoadingOneNoteNotebooks(true);
    try {
      setOneNoteNotebooks(await microsoftExportService.listNotebooks());
      setOneNoteNotebookListingLimited(false);
    } catch (e) {
      if (isOneNoteLargeLibraryError(e)) {
        setOneNoteNotebooks([]);
        setOneNoteNotebookListingLimited(true);
      } else {
        toast.error("Could not load OneNote notebooks", {
          description: errorText(e),
        });
      }
    } finally {
      setLoadingOneNoteNotebooks(false);
    }
  }, []);

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
    })();
    return () => {
      cancelled = true;
    };
  }, [meetingId]);

  useEffect(() => {
    if (!oneNoteOpen || !connected || !showNewSection) return;
    void loadOneNoteNotebooks();
  }, [connected, loadOneNoteNotebooks, oneNoteOpen, showNewSection]);

  const submitNewNotebook = useCallback(async () => {
    const name = sanitizeNotebookName(newNotebookName).trim();
    if (!name) return;
    setSavingNotebook(true);
    try {
      const notebook = await microsoftExportService.createNotebook(name);
      setOneNoteNotebooks((prev) =>
        prev.some((n) => n.id === notebook.id) ? prev : [...prev, notebook],
      );
      setOneNoteNotebookId(notebook.id);
      setOneNoteSavedNotebookName(notebook.displayName);
      setOneNoteNotebookListingLimited(false);
      setCreatingNotebook(false);
      setNewNotebookName("");
    } catch (e) {
      toast.error("Could not create OneNote notebook", {
        description: errorText(e),
      });
    } finally {
      setSavingNotebook(false);
    }
  }, [newNotebookName]);

  const openHistory = useCallback(async () => {
    setHistoryOpen(true);
    setReviewItem(null);
    setHistory(null);
    setHistoryError("");
    try {
      setHistory(await microsoftExportService.getMeetingExportHistory(meetingId));
    } catch (error) {
      setHistoryError(errorText(error));
    }
  }, [meetingId]);

  function selectReview(item: MeetingExportHistory) {
    setReviewItem(item);
    setReviewOutcome("");
    setRemoteId("");
    setReviewError("");
  }

  async function reconcileExport() {
    if (!reviewItem || pending.current || !reviewOutcome || (reviewOutcome === "exists" && !remoteId.trim())) return;
    pending.current = true;
    setSavingReview(true);
    setReviewError("");
    try {
      await microsoftExportService.reconcileMeetingExport(meetingId, reviewItem.entryKey,
        reviewOutcome === "exists" ? remoteId.trim() : null);
      await openHistory();
    } catch (error) {
      setReviewError(errorText(error));
    } finally {
      pending.current = false;
      setSavingReview(false);
    }
  }

  const reportToast = useCallback(
    (
      label: string,
      report: {
        overall: string;
        items: Array<{ webUrl: string | null; code: string | null; status: string; graphCalled: boolean }>;
      },
    ) => {
      const webUrl = report.items.find((i) => i.webUrl)?.webUrl ?? null;
      // The backend serializes ExportStatus via Debug-lowercase, so success is
      // "succeeded" (not "success").
      if (report.overall === "succeeded") {
        const created = report.items.filter(item => item.graphCalled).length;
        const existing = report.items.length - created;
        toast.success(`${label} export complete`, {
          description: [created > 0 && `${created} created`, existing > 0 && `${existing} already exported`].filter(Boolean).join(" · "),
          action: webUrl
            ? { label: "Open", onClick: () => openExternal(webUrl) }
            : undefined,
        });
      } else {
        const description = report.items.some(item => item.status === "unknownaftersubmit")
          ? "Check the destination and resolve the outcome in Export history."
          : report.items.some(item => item.code === TASK_IDENTITY_REVIEW_CODE)
            ? "Check the tasks marked for review before exporting again."
            : report.items.some(item => item.status === "failedauth")
              ? "Reconnect Microsoft in Settings, then retry."
              : report.items.some(item => item.status === "accessdenied" || item.status === "tenantblocked")
                ? "Check the account’s access to this destination in Microsoft 365."
                : report.items.some(item => item.status === "destinationnotfound")
                  ? "Choose an available destination in Microsoft 365 settings."
                  : "Open export history to review the results.";
        toast.warning(`${label} export finished with issues`, {
          description,
          action: { label: "View history", onClick: () => void openHistory() },
        });
      }
    },
    [openHistory],
  );

  const loadResumes = useCallback(async () => {
    setResumeError("");
    setResumes(null);
    try {
      const available = await microsoftExportService.listOneNoteResumes(meetingId);
      setResumeId(available[0]?.id ?? "new");
      setResumes(available);
    } catch (error) {
      setResumeError(errorText(error));
    }
  }, [meetingId]);

  const openOneNote = useCallback(async () => {
    setExportOptions(defaultExportOptions());
    setReplaceOneNotePage(false);
    setOneNoteDraft(null);
    const saved = getExportDestinations();
    const generatedTitle = defaultDatedTitle(meetingTitle, meetingCreatedAt);
    setOneNoteNotebookId(saved.notebookId ?? "");
    setOneNoteSavedNotebookName(saved.notebookName ?? null);
    setOneNoteSectionName(sanitizeSectionName(generatedTitle));
    setOneNotePageTitle(generatedTitle);
    setOneNoteNotebookListingLimited(false);
    setCreatingNotebook(false);
    setNewNotebookName("");
    setOneNoteOpen(true);
    await loadResumes();
  }, [loadResumes, meetingCreatedAt, meetingTitle]);

  const confirmOneNote = useCallback(async () => {
    if (pending.current || !resumes || selectedResume?.canResume === false) return;
    const replaceExisting = !!selectedResume && replaceOneNotePage;
    const pageTitle = selectedResume && !replaceExisting
      ? selectedResume.pageTitle
      : oneNotePageTitle.trim() || defaultDatedTitle(meetingTitle, meetingCreatedAt);
    const sectionName = sanitizeSectionName(oneNoteSectionName).trim();
    if (!selectedResume && (!oneNoteNotebookId || !sectionName)) return;
    const options = replaceExisting ? exportOptions : selectedResume?.options ?? exportOptions;
    pending.current = true;
    setBusy("onenote");
    try {
      const md = replaceExisting && oneNoteDraft !== null ? oneNoteDraft
        : (await prepareMeetingExport(meetingId, getMarkdown, options)).markdown;
      if (replaceExisting && oneNoteDraft === null) {
        setOneNoteDraft(md);
        return;
      }
      if (!selectedResume) {
        const notebookName = oneNoteNotebooks.find(n => n.id === oneNoteNotebookId)?.displayName ?? oneNoteSavedNotebookName ?? undefined;
        setExportDestinations({ notebookId: oneNoteNotebookId, notebookName, sectionId: undefined, sectionName: undefined });
      }
      const report = await microsoftExportService.exportMeetingToOneNoteSection(
        meetingId, pageTitle, md, oneNoteNotebookId, sectionName, options, selectedResume?.id ?? null, replaceExisting,
      );
      if (replaceExisting && report.overall === "succeeded") toast.success("OneNote export saved");
      else reportToast("OneNote", report);
      if (report.overall === "succeeded") setOneNoteOpen(false);
      else await loadResumes();
    } catch (error) {
      const description = errorText(error);
      toast.error("OneNote export failed", { description, action: /export history/i.test(description)
        ? { label: "View history", onClick: () => { setOneNoteOpen(false); void openHistory(); } }
        : undefined });
      // Section creation may already have been checkpointed before the failure.
      if (!selectedResume) await loadResumes();
    } finally {
      pending.current = false;
      setBusy(null);
    }
  }, [getMarkdown, exportOptions, loadResumes, meetingCreatedAt, meetingId, meetingTitle, oneNoteNotebookId,
    oneNoteNotebooks, oneNoteSavedNotebookName, oneNotePageTitle, oneNoteSectionName, reportToast, resumes, selectedResume,
    replaceOneNotePage, oneNoteDraft, openHistory]);

  const selectedNotebookMissing =
    !!oneNoteNotebookId && !oneNoteNotebooks.some((n) => n.id === oneNoteNotebookId);

  // Planner export opens a review dialog (pick/edit/route action items) rather
  // than creating tasks immediately.
  const openPlanner = useCallback(() => {
    if (!hasPlannerDestination(getExportDestinations())) {
      toast.info("Pick a Planner plan and default bucket first", {
        description: "Settings → Microsoft 365 → Planner.",
      });
      return;
    }
    setPlannerOpen(true);
  }, []);

  const openToDo = useCallback(() => {
    if (!hasToDoDestination(getExportDestinations())) {
      toast.info("Pick a Microsoft To Do list first", {
        description: "Settings → Microsoft 365 → To Do.",
      });
      return;
    }
    setToDoOpen(true);
  }, []);

  const exportConfluence = useCallback(async () => {
    if (pending.current) return;
    pending.current = true;
    setBusy("confluence");
    try {
      const { markdown: md } = await prepareMeetingExport(meetingId, getMarkdown, exportOptions);

      const draft = buildConfluenceDraftMarkdown({
        meetingTitle,
        meetingCreatedAt,
        summaryMarkdown: md,
      });
      const destinations = getExportDestinations();
      const {
        confluenceMode = "draft",
        confluenceCreateUrl,
        confluenceOpenAfterCopy = true,
        confluenceBaseUrl,
        confluenceSpaceKey,
        confluenceParentId,
      } = destinations;
      const url = confluenceCreateUrl?.trim();

      if (confluenceMode === "rest") {
        const baseUrl = confluenceBaseUrl?.trim();
        const spaceKey = confluenceSpaceKey?.trim();
        if (!baseUrl || !spaceKey) {
          toast.info("Finish Confluence REST setup first", {
            description: "Settings → Confluence.",
          });
          return;
        }

        try {
          const report = await confluenceExportService.exportPage({
            meetingId,
            baseUrl,
            spaceKey,
            parentId: confluenceParentId?.trim() || null,
            title: defaultConfluencePageTitle(meetingTitle, meetingCreatedAt),
            bodyStorage: markdownToConfluenceHtml(draft),
          });

          setDocumentDestination(null);
          toast.success("Confluence export complete", {
            description: report.title,
            action: report.webUrl
              ? { label: "Open", onClick: () => openExternal(report.webUrl!) }
              : undefined,
          });
          return;
        } catch (restError) {
          if (/export history/i.test(errorText(restError))) {
            setDocumentDestination(null);
            toast.warning("Confluence export needs review", {
              description: errorText(restError),
              action: { label: "View history", onClick: () => void openHistory() },
            });
            return;
          }
          const copyMode = await writeConfluenceDraftToClipboard(draft);
          if (url && confluenceOpenAfterCopy) {
            openExternal(url);
          }
          setDocumentDestination(null);
          toast.error("Confluence REST export failed", {
            description:
              copyMode === "rich"
                ? `${errorText(restError)} Draft copied with rich formatting instead.`
                : `${errorText(restError)} Markdown draft copied instead.`,
          });
          return;
        }
      }

      const mode = await writeConfluenceDraftToClipboard(draft);
      if (url && confluenceOpenAfterCopy) {
        openExternal(url);
      }

      setDocumentDestination(null);
      toast.success("Confluence draft copied", {
        description:
          mode === "rich"
            ? "Paste into the Confluence editor. Rich formatting was copied when supported."
            : "Paste into the Confluence editor. Markdown was copied.",
      });
    } catch (e) {
      toast.error("Confluence export failed", {
        description: errorText(e),
      });
    } finally {
      pending.current = false;
      setBusy(null);
    }
  }, [getMarkdown, exportOptions, meetingCreatedAt, meetingId, meetingTitle, openHistory]);

  const exportOneDrive = useCallback(async () => {
    if (pending.current) return;
    pending.current = true;
    setBusy("onedrive");
    try {
      const { summary, transcript } = await prepareMeetingExport(meetingId, getMarkdown, exportOptions);

      const destinations = getExportDestinations();
      const response = await microsoftExportService.exportMeetingToOneDriveFiles({
        meetingId,
        meetingTitle: meetingCreatedAt ? defaultDatedTitle(meetingTitle, meetingCreatedAt) : meetingTitle,
        markdown: summary,
        transcript,
        destination: destinations.oneDriveDestination ?? null,
        includePdf: destinations.oneDriveIncludePdf ?? true,
        createOrganizationLink: destinations.oneDriveCreateOrganizationLink ?? false,
      });

      const openUrl =
        response.files.find((file) => file.sharingLink)?.sharingLink ??
        response.files.find((file) => file.webUrl)?.webUrl ??
        null;
      const kinds = response.files.map((file) => file.kind.toUpperCase()).join(" + ");
      setDocumentDestination(null);
      if (response.issues?.length) {
        toast.warning(kinds ? "OneDrive export partially complete" : "OneDrive export needs attention", {
          description: [kinds && `${kinds} in ${response.destination.name}.`, ...response.issues].filter(Boolean).join(" "),
          action: { label: "View history", onClick: () => void openHistory() },
        });
        return;
      }
      toast.success("OneDrive export complete", {
        description: `${kinds || "Files"} uploaded to ${response.destination.name}.`,
        action: openUrl
          ? { label: "Open", onClick: () => openExternal(openUrl) }
          : undefined,
      });
    } catch (e) {
      const description = errorText(e);
      toast.error("OneDrive export failed", {
        description,
        action: /export history/i.test(description)
          ? { label: "View history", onClick: () => { setDocumentDestination(null); void openHistory(); } }
          : undefined,
      });
    } finally {
      pending.current = false;
      setBusy(null);
    }
  }, [getMarkdown, exportOptions, meetingCreatedAt, meetingId, meetingTitle, openHistory]);

  const exportWord = async () => {
    if (pending.current) return;
    pending.current = true;
    setBusy("word");
    try {
      const markdown = await readExportSummary(getMarkdown, exportOptions);
      // The core opens the save dialog and only accepts the path it chose here.
      const path = await invoke<string | null>("pick_word_save_path", { suggestedName: safeDocumentName(meetingTitle) });
      if (!path) return;
      await invoke("export_local_word", {
        meetingId, title: meetingTitle, markdown, path,
        includeTranscript: exportOptions.content !== "summary",
        includeSpeakers: exportOptions.speakers,
        includeTimestamps: exportOptions.timestamps,
      });
      toast.success("Word document saved");
      setDocumentDestination(null);
    } catch (error) {
      toast.error("Word export failed", { description: errorText(error) });
    } finally {
      pending.current = false;
      setBusy(null);
    }
  };

  return (
    <div className="flex items-center gap-2">
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button type="button" variant="outline" size="sm" aria-label="Export meeting" disabled={disabled || busy !== null}>
            {busy !== null ? (
              <Loader2 className="h-4 w-4 animate-spin 2xl:mr-2" />
            ) : (
              <Upload className="h-4 w-4 2xl:mr-2" />
            )}
            <span className="hidden 2xl:inline">Export</span>
            <ChevronDown className="h-3.5 w-3.5 opacity-70 2xl:ml-1.5" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-52">
          <DropdownMenuLabel>Export meeting to</DropdownMenuLabel>
          <DropdownMenuItem onClick={() => openDocumentExport("word")}>
            <WordIcon className="mr-2 h-4 w-4" />
            Word document (.docx)
          </DropdownMenuItem>
          {connected && (
            <DropdownMenuItem onClick={openOneNote}>
              <OneNoteIcon className="mr-2 h-4 w-4" />
              OneNote
            </DropdownMenuItem>
          )}
          {connected && (
            <DropdownMenuItem onClick={() => openDocumentExport("onedrive")}>
              <OneDriveIcon className="mr-2 h-4 w-4" />
              OneDrive DOCX/PDF
            </DropdownMenuItem>
          )}
          {connected && (
            <DropdownMenuItem onClick={openPlanner}>
              <PlannerIcon className="mr-2 h-4 w-4" />
              Planner action items
            </DropdownMenuItem>
          )}
          {connected && (
            <DropdownMenuItem onClick={openToDo}>
              <ToDoIcon className="mr-2 h-4 w-4" />
              Microsoft To Do tasks
            </DropdownMenuItem>
          )}
          <DropdownMenuItem onClick={() => openDocumentExport("confluence")}>
            <ConfluenceIcon className="mr-2 h-4 w-4" />
            Confluence
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => void openHistory()}>Export history</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <Dialog open={documentDestination !== null} onOpenChange={open => { if (!open && !pending.current) setDocumentDestination(null); }}>
        <DialogContent className="max-h-[85vh] overflow-y-auto">
          <DialogHeader>
            <DialogTitle>{documentDestination && documentLabels[documentDestination].title}</DialogTitle>
            <DialogDescription>{documentDestination && documentLabels[documentDestination].description}</DialogDescription>
          </DialogHeader>
          <ExportContentOptions value={exportOptions} onChange={setExportOptions} disabled={busy !== null} />
          <DialogFooter>
            <Button variant="outline" disabled={busy !== null} onClick={() => setDocumentDestination(null)}>Cancel</Button>
            <Button disabled={busy !== null} onClick={documentDestination === "word" ? exportWord : documentDestination === "onedrive" ? exportOneDrive : exportConfluence}>
              {busy !== null ? "Exporting…" : documentDestination && documentLabels[documentDestination].submit}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={historyOpen} onOpenChange={open => { if (!savingReview) setHistoryOpen(open); }}>
        <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>Export history</DialogTitle>
            <DialogDescription>Remote exports from this meeting and any results that need review.</DialogDescription>
          </DialogHeader>
          {historyError && <div className="space-y-2">
            <p role="alert" className="text-sm text-destructive">{historyError}</p>
            <Button type="button" variant="outline" onClick={() => void openHistory()}>Retry loading</Button>
          </div>}
          {!history && !historyError && <p role="status" className="text-sm text-muted-foreground">Loading export history…</p>}
          {history?.length === 0 && <p className="text-sm text-muted-foreground">No remote exports recorded for this meeting.</p>}
          {reviewItem && <form className="space-y-3" onSubmit={event => { event.preventDefault(); void reconcileExport(); }}>
            <p className="text-sm">Check {reviewItem.destination} for {reviewItem.displayName ? `“${reviewItem.displayName}”` : "the exported item"}, then record what you found.</p>
            <Label htmlFor="export-review-outcome">What did you find?</Label>
            <Select value={reviewOutcome} onValueChange={setReviewOutcome} disabled={savingReview}>
              <SelectTrigger id="export-review-outcome"><SelectValue placeholder="Choose an outcome" /></SelectTrigger>
              <SelectContent>
                <SelectItem value="exists">I found the exported item</SelectItem>
                <SelectItem value="absent">I confirmed the item is missing</SelectItem>
              </SelectContent>
            </Select>
            {reviewOutcome === "exists" && <div className="space-y-2">
              <Label htmlFor="export-review-id">ID of the existing item</Label>
              <Input id="export-review-id" maxLength={512} value={remoteId} onChange={event => setRemoteId(event.target.value)} disabled={savingReview} />
            </div>}
            {reviewError && <p role="alert" className="text-sm text-destructive">{reviewError}</p>}
            <div className="flex justify-end gap-2">
              <Button type="button" variant="outline" onClick={() => setReviewItem(null)} disabled={savingReview}>Cancel</Button>
              <Button type="submit" disabled={savingReview || !reviewOutcome || (reviewOutcome === "exists" && !remoteId.trim())}>
                {reviewOutcome === "absent" ? "Allow retry" : "Save outcome"}
              </Button>
            </div>
          </form>}
          {!reviewItem && !!history?.length && <ul className="divide-y divide-border">
            {history.map((item, index) => <li key={item.entryKey ?? index} className="flex items-center gap-3 py-2 text-sm">
              <div className="flex-1">
                <p>{item.destination} · {historyStatus(item.status)}</p>
                {item.displayName && <p className="break-words text-xs text-muted-foreground">{item.displayName}</p>}
                {item.at && <time className="text-xs text-muted-foreground" dateTime={item.at}>{new Date(item.at).toLocaleString()}</time>}
              </div>
              {item.webUrl && <Button type="button" variant="outline" size="sm" aria-label={`Open in ${item.destination}`} onClick={() => void openExternal(item.webUrl!)}>Open</Button>}
              {item.status === "unknown_after_submit" && <Button type="button" variant="outline" size="sm" onClick={() => selectReview(item)}>Resolve outcome</Button>}
            </li>)}
          </ul>}
        </DialogContent>
      </Dialog>

      <Dialog open={oneNoteOpen} onOpenChange={(o) => !busy && setOneNoteOpen(o)}>
        <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>Export to OneNote</DialogTitle>
            <DialogDescription>
              Resume in a saved section or export to a new section.
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4">
            {resumeError && <div className="space-y-2">
              <p role="alert" className="text-sm text-destructive">{resumeError}</p>
              <Button type="button" variant="outline" onClick={() => void loadResumes()}>Retry loading</Button>
            </div>}
            {!resumes && !resumeError && <p role="status" className="text-sm text-muted-foreground">Loading saved sections…</p>}
            {!!resumes?.length && <div className="space-y-2">
              <Label htmlFor="onenote-export-section">Section</Label>
              <Select value={resumeId} onValueChange={id => { setResumeId(id); setReplaceOneNotePage(false); setOneNoteDraft(null); }} disabled={busy === "onenote"}>
                <SelectTrigger id="onenote-export-section"><SelectValue /></SelectTrigger>
                <SelectContent>
                  {resumes.map(attempt => <SelectItem key={attempt.id} value={attempt.id}>{attempt.sectionName} · {new Date(attempt.startedAt).toLocaleString()}</SelectItem>)}
                  <SelectItem value="new">Create another section</SelectItem>
                </SelectContent>
              </Select>
            </div>}
            {selectedResume?.canResume && <div className="space-y-2">
              <Label htmlFor="onenote-saved-page-action">Saved page action</Label>
              <Select value={replaceOneNotePage ? "update" : "resume"} onValueChange={value => {
                setReplaceOneNotePage(value === "update");
                setOneNoteDraft(null);
                setOneNotePageTitle(selectedResume.pageTitle);
                setExportOptions(selectedResume.options ?? defaultExportOptions());
              }} disabled={busy === "onenote"}>
                <SelectTrigger id="onenote-saved-page-action"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="resume">Resume export</SelectItem>
                  <SelectItem value="update">Update exported page</SelectItem>
                </SelectContent>
              </Select>
            </div>}
            {selectedResume && !selectedResume.canResume && <p className="text-sm text-muted-foreground">Check OneNote and resolve this section in Export history.</p>}
            {selectedResume && replaceOneNotePage && <>
              <Label htmlFor="onenote-updated-title">Page title</Label>
              <Input id="onenote-updated-title" value={oneNotePageTitle} onChange={event => setOneNotePageTitle(event.target.value)} disabled={busy === "onenote"} />
              <ExportContentOptions value={exportOptions} onChange={options => { setExportOptions(options); setOneNoteDraft(null); }} disabled={busy !== null} />
              {oneNoteDraft !== null && <>
                <Label htmlFor="onenote-page-draft">Review page content</Label>
                <Textarea id="onenote-page-draft" value={oneNoteDraft} onChange={event => setOneNoteDraft(event.target.value)} disabled={busy === "onenote"} rows={8} />
                <p className="text-xs text-muted-foreground">Updating replaces the exported content in the saved page. Older exports may receive a revised page in the same section.</p>
              </>}
            </>}
            {showNewSection && <>
              <ExportContentOptions value={exportOptions} onChange={setExportOptions} disabled={busy !== null} />
              <div className="space-y-2">
                <div className="flex items-center justify-between">
                  <Label htmlFor="onenote-notebook">Notebook</Label>
                  <Button variant="ghost"
                    type="button"
                    className="h-auto gap-0 rounded-none p-0 text-xs font-normal text-muted-foreground hover:bg-transparent hover:text-foreground disabled:opacity-50"
                    onClick={() => void loadOneNoteNotebooks()}
                    disabled={loadingOneNoteNotebooks || busy === "onenote"}
                  >
                    {loadingOneNoteNotebooks ? "Loading..." : "Reload"}
                  </Button>
                </div>
                <Select
                  value={creatingNotebook ? "__new__" : oneNoteNotebookId || EMPTY_NOTEBOOK_VALUE}
                  onValueChange={(value) => {
                    if (value === "__new__") {
                      focusNewNotebookOnClose.current = true;
                      setCreatingNotebook(true);
                      return;
                    }
                    setCreatingNotebook(false);
                    setOneNoteNotebookId(value === EMPTY_NOTEBOOK_VALUE ? "" : value);
                    setOneNoteSavedNotebookName(null);
                  }}
                  disabled={loadingOneNoteNotebooks || busy === "onenote"}
                >
                  <SelectTrigger id="onenote-notebook" className="h-auto w-full rounded-lg border border-border bg-background px-3 py-2 text-sm shadow-none"><SelectValue /></SelectTrigger>
                  <SelectContent onCloseAutoFocus={(event) => {
                    if (focusNewNotebookOnClose.current) {
                      focusNewNotebookOnClose.current = false;
                      event.preventDefault();
                      newNotebookInputRef.current?.focus();
                    }
                  }}>
                    <SelectItem value={EMPTY_NOTEBOOK_VALUE}>
                      {loadingOneNoteNotebooks ? "Loading..." : "Select a notebook"}
                    </SelectItem>
                    {oneNoteNotebooks.map((notebook) => (
                      <SelectItem key={notebook.id} value={notebook.id}>
                        {notebook.displayName}
                      </SelectItem>
                    ))}
                    {selectedNotebookMissing && (
                      <SelectItem value={oneNoteNotebookId}>
                        {oneNoteSavedNotebookName ?? "Saved notebook"}
                      </SelectItem>
                    )}
                    <SelectItem value="__new__">+ New notebook...</SelectItem>
                  </SelectContent>
                </Select>
              </div>

              {oneNoteNotebookListingLimited && (
                <p className="rounded-lg border border-warning-border/30 bg-warning p-3 text-sm text-warning-foreground">
                  {ONENOTE_LARGE_LIBRARY_MESSAGE}
                </p>
              )}

              {creatingNotebook && (
                <div className="space-y-2 rounded-lg border border-border bg-muted p-3">
                  <Label htmlFor="onenote-new-notebook">New notebook name</Label>
                  <div className="flex items-center gap-2">
                    <Input
                      id="onenote-new-notebook"
                      ref={newNotebookInputRef}
                      value={newNotebookName}
                      onChange={(e) => setNewNotebookName(sanitizeNotebookName(e.target.value))}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") void submitNewNotebook();
                        if (e.key === "Escape") setCreatingNotebook(false);
                      }}
                      maxLength={ONENOTE_NOTEBOOK_MAX}
                      autoFocus
                    />
                    <Button
                      type="button"
                      size="sm"
                      onClick={() => void submitNewNotebook()}
                      disabled={savingNotebook || !newNotebookName.trim()}
                    >
                      {savingNotebook ? <Loader2 className="h-4 w-4 animate-spin" /> : "Create"}
                    </Button>
                  </div>
                </div>
              )}

              <div className="space-y-2">
                <Label htmlFor="onenote-section-name">New section name</Label>
                <Input
                  id="onenote-section-name"
                  value={oneNoteSectionName}
                  onChange={(e) => setOneNoteSectionName(sanitizeSectionName(e.target.value))}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && oneNoteNotebookId && oneNoteSectionName.trim()) {
                      void confirmOneNote();
                    }
                  }}
                  maxLength={ONENOTE_SECTION_MAX}
                  disabled={busy === "onenote"}
                />
                <p className="text-xs text-muted-foreground">
                  A fresh OneNote section is created for this export. Invalid
                  OneNote characters are removed automatically. Max 49 characters.
                </p>
              </div>

              <div className="space-y-2">
                <Label htmlFor="onenote-page-title">Page title</Label>
                <Input
                  id="onenote-page-title"
                  value={oneNotePageTitle}
                  onChange={(e) => setOneNotePageTitle(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && oneNoteNotebookId && oneNoteSectionName.trim()) {
                      void confirmOneNote();
                    }
                  }}
                  disabled={busy === "onenote"}
                />
              </div>
            </>}
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
            <Button
              type="button"
              onClick={confirmOneNote}
              disabled={busy === "onenote" || !resumes || selectedResume?.canResume === false || (replaceOneNotePage && oneNoteDraft !== null && !oneNoteDraft.trim()) || (!selectedResume && (!oneNoteNotebookId || !oneNoteSectionName.trim()))}
            >
              {busy === "onenote" && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
              {selectedResume ? (replaceOneNotePage ? (oneNoteDraft === null ? "Review changes" : "Update exported page") : "Resume export") : resumes?.length ? "Create another section" : "Export page"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {connected && (
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
          onReviewActions={onReviewActions}
          onReport={(report) => reportToast("Planner", report)}
        />
      )}
      {connected && (
        <ToDoExportPreview
          open={todoOpen}
          onOpenChange={(o) => !busy && setToDoOpen(o)}
          meetingId={meetingId}
          meetingTitle={meetingTitle}
          listId={getExportDestinations().todoListId ?? ""}
          listName={getExportDestinations().todoListName}
          getMarkdown={getMarkdown}
          onReviewActions={onReviewActions}
          onReport={(report) => reportToast("Microsoft To Do", report)}
        />
      )}
    </div>
  );
}
