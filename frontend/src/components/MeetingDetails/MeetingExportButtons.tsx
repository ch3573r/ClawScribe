"use client";

import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import { FileText, NotebookTabs, ListTodo, Loader2, Upload, ChevronDown } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
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
  ONENOTE_LARGE_LIBRARY_MESSAGE,
  isOneNoteLargeLibraryError,
  microsoftExportService,
  type MicrosoftConnectionInfo,
  type NotebookInfo,
  type SectionInfo,
} from "@/services/microsoftExportService";
import {
  buildConfluenceDraftMarkdown,
  markdownToConfluenceHtml,
  writeConfluenceDraftToClipboard,
} from "@/lib/confluenceDraft";
import {
  getExportDestinations,
  hasPlannerDestination,
  setExportDestinations,
} from "@/lib/exportDestinations";
import { confluenceExportService } from "@/services/confluenceExportService";
import { PlannerExportPreview } from "./PlannerExportPreview";

interface MeetingExportButtonsProps {
  meetingId: string;
  meetingTitle: string;
  meetingCreatedAt?: string;
  /** Resolves the current summary as markdown. */
  getMarkdown: () => Promise<string>;
}

type Busy = "onenote" | "planner" | "confluence" | null;

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
  const stamp = Number.isNaN(date.getTime())
    ? new Date().toISOString().slice(0, 10)
    : date.toISOString().slice(0, 10);
  const clean = (title || "Untitled meeting").trim();
  return `${stamp} ${clean}`.slice(0, 240).trim();
}

function defaultConfluencePageTitle(title: string, createdAt?: string): string {
  return defaultDatedTitle(title, createdAt);
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Per-meeting export actions shown in the summary view. OneNote is always
 * available once Microsoft is connected; exporting opens a dialog to choose the
 * notebook and section where a new page will be created. Planner appears only
 * when the summary has action items.
 */
export function MeetingExportButtons({
  meetingId,
  meetingTitle,
  meetingCreatedAt,
  getMarkdown,
}: MeetingExportButtonsProps) {
  const [connected, setConnected] = useState(false);
  const [hasActionItems, setHasActionItems] = useState(false);
  const [busy, setBusy] = useState<Busy>(null);

  const [oneNoteOpen, setOneNoteOpen] = useState(false);
  const [plannerOpen, setPlannerOpen] = useState(false);
  const [oneNoteNotebooks, setOneNoteNotebooks] = useState<NotebookInfo[]>([]);
  const [oneNoteSections, setOneNoteSections] = useState<SectionInfo[]>([]);
  const [oneNoteNotebookId, setOneNoteNotebookId] = useState("");
  const [oneNoteSectionId, setOneNoteSectionId] = useState("");
  const [oneNoteSavedNotebookName, setOneNoteSavedNotebookName] = useState<string | null>(null);
  const [oneNoteSavedSectionName, setOneNoteSavedSectionName] = useState<string | null>(null);
  const [oneNotePageTitle, setOneNotePageTitle] = useState("");
  const [loadingOneNoteNotebooks, setLoadingOneNoteNotebooks] = useState(false);
  const [loadingOneNoteSections, setLoadingOneNoteSections] = useState(false);
  const [oneNoteNotebookListingLimited, setOneNoteNotebookListingLimited] = useState(false);
  const [oneNoteSectionListingLimited, setOneNoteSectionListingLimited] = useState(false);
  const [creatingNotebook, setCreatingNotebook] = useState(false);
  const [creatingSection, setCreatingSection] = useState(false);
  const [newNotebookName, setNewNotebookName] = useState("");
  const [newSectionName, setNewSectionName] = useState("");
  const [savingNotebook, setSavingNotebook] = useState(false);
  const [savingSection, setSavingSection] = useState(false);

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

  const loadOneNoteSections = useCallback(async (notebookId: string) => {
    if (!notebookId) {
      setOneNoteSections([]);
      return;
    }
    setLoadingOneNoteSections(true);
    try {
      setOneNoteSections(await microsoftExportService.listSections(notebookId));
      setOneNoteSectionListingLimited(false);
    } catch (e) {
      setOneNoteSections([]);
      if (isOneNoteLargeLibraryError(e)) {
        setOneNoteSectionListingLimited(true);
      } else {
        toast.error("Could not load OneNote sections", {
          description: errorText(e),
        });
      }
    } finally {
      setLoadingOneNoteSections(false);
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

  useEffect(() => {
    if (!oneNoteOpen || !connected) return;
    void loadOneNoteNotebooks();
  }, [connected, loadOneNoteNotebooks, oneNoteOpen]);

  useEffect(() => {
    if (!oneNoteOpen || !oneNoteNotebookId) return;
    void loadOneNoteSections(oneNoteNotebookId);
  }, [loadOneNoteSections, oneNoteNotebookId, oneNoteOpen]);

  useEffect(() => {
    if (
      oneNoteOpen &&
      oneNoteNotebookId &&
      oneNoteSectionListingLimited &&
      !oneNoteSectionId &&
      !creatingSection
    ) {
      setCreatingSection(true);
      setNewSectionName((prev) =>
        prev || sanitizeSectionName(defaultDatedTitle(meetingTitle, meetingCreatedAt)),
      );
    }
  }, [
    creatingSection,
    meetingCreatedAt,
    meetingTitle,
    oneNoteNotebookId,
    oneNoteOpen,
    oneNoteSectionId,
    oneNoteSectionListingLimited,
  ]);

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
      setOneNoteSectionId("");
      setOneNoteSavedSectionName(null);
      setOneNoteNotebookListingLimited(false);
      setOneNoteSectionListingLimited(false);
      setCreatingNotebook(false);
      setNewNotebookName("");
      setOneNoteSections([]);
    } catch (e) {
      toast.error("Could not create OneNote notebook", {
        description: errorText(e),
      });
    } finally {
      setSavingNotebook(false);
    }
  }, [newNotebookName]);

  const submitNewSection = useCallback(async () => {
    const name = sanitizeSectionName(newSectionName).trim();
    if (!name || !oneNoteNotebookId) return;
    setSavingSection(true);
    try {
      const section = await microsoftExportService.createSection(
        oneNoteNotebookId,
        name,
      );
      setOneNoteSections((prev) =>
        prev.some((s) => s.id === section.id) ? prev : [...prev, section],
      );
      setOneNoteSectionId(section.id);
      setOneNoteSavedSectionName(section.displayName);
      setOneNoteSectionListingLimited(false);
      setCreatingSection(false);
      setNewSectionName("");
    } catch (e) {
      toast.error("Could not create OneNote section", {
        description: errorText(e),
      });
    } finally {
      setSavingSection(false);
    }
  }, [newSectionName, oneNoteNotebookId]);

  // Opening the OneNote dialog: load the saved destination if present, then let
  // the user override it or create a notebook/section inline.
  const openOneNote = useCallback(() => {
    const saved = getExportDestinations();
    setOneNoteNotebookId(saved.notebookId ?? "");
    setOneNoteSectionId(saved.sectionId ?? "");
    setOneNoteSavedNotebookName(saved.notebookName ?? null);
    setOneNoteSavedSectionName(saved.sectionName ?? null);
    setOneNotePageTitle(defaultDatedTitle(meetingTitle, meetingCreatedAt));
    setOneNoteNotebookListingLimited(false);
    setOneNoteSectionListingLimited(false);
    setCreatingNotebook(false);
    setCreatingSection(false);
    setNewNotebookName("");
    setNewSectionName("");
    setOneNoteOpen(true);
  }, [meetingCreatedAt, meetingTitle]);

  const confirmOneNote = useCallback(async () => {
    const pageTitle = oneNotePageTitle.trim() || defaultDatedTitle(meetingTitle, meetingCreatedAt);
    if (!oneNoteNotebookId) {
      toast.info("Choose a OneNote notebook.");
      return;
    }
    if (!oneNoteSectionId) {
      toast.info("Choose or create a OneNote section.");
      return;
    }
    setBusy("onenote");
    try {
      const md = await getMarkdown();
      if (!md.trim()) {
        toast.info("Nothing to export yet — generate a summary first.");
        return;
      }
      const notebookName =
        oneNoteNotebooks.find((n) => n.id === oneNoteNotebookId)?.displayName ??
        oneNoteSavedNotebookName ??
        undefined;
      const sectionName =
        oneNoteSections.find((s) => s.id === oneNoteSectionId)?.displayName ??
        oneNoteSavedSectionName ??
        undefined;
      setExportDestinations({
        notebookId: oneNoteNotebookId,
        notebookName,
        sectionId: oneNoteSectionId,
        sectionName,
      });
      const report = await microsoftExportService.exportMeetingMarkdownToOneNote(
        meetingId,
        pageTitle,
        md,
        oneNoteSectionId,
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
  }, [
    getMarkdown,
    meetingCreatedAt,
    meetingId,
    meetingTitle,
    oneNoteNotebookId,
    oneNoteNotebooks,
    oneNoteSavedNotebookName,
    oneNoteSavedSectionName,
    oneNotePageTitle,
    oneNoteSectionId,
    oneNoteSections,
    reportToast,
  ]);

  const selectedNotebookMissing =
    !!oneNoteNotebookId && !oneNoteNotebooks.some((n) => n.id === oneNoteNotebookId);
  const selectedSectionMissing =
    !!oneNoteSectionId && !oneNoteSections.some((s) => s.id === oneNoteSectionId);

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

  const exportConfluence = useCallback(async () => {
    setBusy("confluence");
    try {
      const md = await getMarkdown();
      if (!md.trim()) {
        toast.info("Nothing to export yet — generate a summary first.");
        return;
      }

      const draft = buildConfluenceDraftMarkdown({
        meetingId,
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
            description: "Settings → Add-ons → Confluence export.",
          });
          return;
        }

        try {
          const report = await confluenceExportService.exportPage({
            baseUrl,
            spaceKey,
            parentId: confluenceParentId?.trim() || null,
            title: defaultConfluencePageTitle(meetingTitle, meetingCreatedAt),
            bodyStorage: markdownToConfluenceHtml(draft),
          });

          toast.success("Confluence export complete", {
            description: report.webUrl ? "Page created in Confluence." : report.title,
            action: report.webUrl
              ? { label: "Open", onClick: () => window.open(report.webUrl!, "_blank") }
              : undefined,
          });
          return;
        } catch (restError) {
          const copyMode = await writeConfluenceDraftToClipboard(draft);
          if (url && confluenceOpenAfterCopy) {
            window.open(url, "_blank");
          }
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
        window.open(url, "_blank");
      }

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
      setBusy(null);
    }
  }, [getMarkdown, meetingCreatedAt, meetingId, meetingTitle]);

  return (
    <div className="flex items-center gap-2">
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button type="button" variant="outline" size="sm" disabled={busy !== null}>
            {busy !== null ? (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            ) : (
              <Upload className="mr-2 h-4 w-4" />
            )}
            Export
            <ChevronDown className="ml-1.5 h-3.5 w-3.5 opacity-70" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="w-52">
          <DropdownMenuLabel>Export summary to</DropdownMenuLabel>
          {connected && (
            <DropdownMenuItem onClick={openOneNote}>
              <NotebookTabs className="mr-2 h-4 w-4" />
              OneNote
            </DropdownMenuItem>
          )}
          {connected && hasActionItems && (
            <DropdownMenuItem onClick={openPlanner}>
              <ListTodo className="mr-2 h-4 w-4" />
              Planner action items
            </DropdownMenuItem>
          )}
          <DropdownMenuItem onClick={exportConfluence}>
            <FileText className="mr-2 h-4 w-4" />
            Confluence
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <Dialog open={oneNoteOpen} onOpenChange={(o) => !busy && setOneNoteOpen(o)}>
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>Export to OneNote</DialogTitle>
            <DialogDescription>
              Create a new page in an existing section, or create a notebook or
              section before exporting.
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4">
            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <Label htmlFor="onenote-notebook">Notebook</Label>
                <button
                  type="button"
                  className="text-xs text-muted-foreground hover:text-foreground disabled:opacity-50"
                  onClick={() => void loadOneNoteNotebooks()}
                  disabled={loadingOneNoteNotebooks || busy === "onenote"}
                >
                  {loadingOneNoteNotebooks ? "Loading..." : "Reload"}
                </button>
              </div>
              <select
                id="onenote-notebook"
                className="w-full rounded-lg border border-border bg-background px-3 py-2 text-sm"
                value={creatingNotebook ? "__new__" : oneNoteNotebookId}
                onChange={(e) => {
                  if (e.target.value === "__new__") {
                    setCreatingNotebook(true);
                    return;
                  }
                  setCreatingNotebook(false);
                  setOneNoteNotebookId(e.target.value);
                  setOneNoteSavedNotebookName(null);
                  setOneNoteSectionId("");
                  setOneNoteSavedSectionName(null);
                  setOneNoteSectionListingLimited(false);
                  setCreatingSection(false);
                }}
                disabled={loadingOneNoteNotebooks || busy === "onenote"}
              >
                <option value="">
                  {loadingOneNoteNotebooks ? "Loading..." : "Select a notebook"}
                </option>
                {oneNoteNotebooks.map((notebook) => (
                  <option key={notebook.id} value={notebook.id}>
                    {notebook.displayName}
                  </option>
                ))}
                {selectedNotebookMissing && (
                  <option value={oneNoteNotebookId}>
                    {oneNoteSavedNotebookName ?? "Saved notebook"}
                  </option>
                )}
                <option value="__new__">+ New notebook...</option>
              </select>
            </div>

            {oneNoteNotebookListingLimited && (
              <p className="rounded-lg border border-amber-500/30 bg-amber-500/10 p-3 text-sm text-amber-800 dark:text-amber-200">
                {ONENOTE_LARGE_LIBRARY_MESSAGE}
              </p>
            )}

            {creatingNotebook && (
              <div className="space-y-2 rounded-lg border border-border bg-muted p-3">
                <Label htmlFor="onenote-new-notebook">New notebook name</Label>
                <div className="flex items-center gap-2">
                  <Input
                    id="onenote-new-notebook"
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
              <div className="flex items-center justify-between">
                <Label htmlFor="onenote-section">Section</Label>
                <button
                  type="button"
                  className="text-xs text-muted-foreground hover:text-foreground disabled:opacity-50"
                  onClick={() => oneNoteNotebookId && void loadOneNoteSections(oneNoteNotebookId)}
                  disabled={!oneNoteNotebookId || loadingOneNoteSections || busy === "onenote"}
                >
                  {loadingOneNoteSections ? "Loading..." : "Reload"}
                </button>
              </div>
              <select
                id="onenote-section"
                className="w-full rounded-lg border border-border bg-background px-3 py-2 text-sm"
                value={creatingSection ? "__new__" : oneNoteSectionId}
                onChange={(e) => {
                  if (e.target.value === "__new__") {
                    setCreatingSection(true);
                    return;
                  }
                  setCreatingSection(false);
                  setOneNoteSectionId(e.target.value);
                  setOneNoteSavedSectionName(null);
                }}
                disabled={!oneNoteNotebookId || loadingOneNoteSections || busy === "onenote"}
              >
                <option value="">
                  {!oneNoteNotebookId
                    ? "Select a notebook first"
                    : loadingOneNoteSections
                      ? "Loading..."
                      : "Select a section"}
                </option>
                {oneNoteSections.map((section) => (
                  <option key={section.id} value={section.id}>
                    {section.displayName}
                  </option>
                ))}
                {selectedSectionMissing && (
                  <option value={oneNoteSectionId}>
                    {oneNoteSavedSectionName ?? "Saved section"}
                  </option>
                )}
                {oneNoteNotebookId && <option value="__new__">+ New section...</option>}
              </select>
            </div>

            {oneNoteSectionListingLimited && oneNoteNotebookId && (
              <p className="rounded-lg border border-amber-500/30 bg-amber-500/10 p-3 text-sm text-amber-800 dark:text-amber-200">
                Microsoft Graph cannot list existing sections because the
                backing OneDrive/SharePoint library hit Graph's OneNote query
                limit. The selected notebook can still have only a few sections.
                Create a new section here, or export to the saved section if one
                is already selected.
              </p>
            )}

            {creatingSection && (
              <div className="space-y-2 rounded-lg border border-border bg-muted p-3">
                <Label htmlFor="onenote-new-section">New section name</Label>
                <div className="flex items-center gap-2">
                  <Input
                    id="onenote-new-section"
                    value={newSectionName}
                    onChange={(e) => setNewSectionName(sanitizeSectionName(e.target.value))}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") void submitNewSection();
                      if (e.key === "Escape") setCreatingSection(false);
                    }}
                    maxLength={ONENOTE_SECTION_MAX}
                    autoFocus
                  />
                  <Button
                    type="button"
                    size="sm"
                    onClick={() => void submitNewSection()}
                    disabled={savingSection || !newSectionName.trim()}
                  >
                    {savingSection ? <Loader2 className="h-4 w-4 animate-spin" /> : "Create"}
                  </Button>
                </div>
              </div>
            )}

            <div className="space-y-2">
              <Label htmlFor="onenote-page-title">Page title</Label>
              <Input
                id="onenote-page-title"
                value={oneNotePageTitle}
                onChange={(e) => setOneNotePageTitle(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && oneNoteNotebookId && oneNoteSectionId) {
                    void confirmOneNote();
                  }
                }}
                disabled={busy === "onenote"}
              />
            </div>
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
              Export page
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
          onReport={(report) => reportToast("Planner", report)}
        />
      )}
    </div>
  );
}
