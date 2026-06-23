"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ElementType, ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Activity,
  AlertTriangle,
  CheckCircle2,
  Copy,
  Cpu,
  Gauge,
  HardDrive,
  Loader2,
  LogIn,
  LogOut,
  Mic,
  RefreshCw,
  ShieldCheck,
  Zap,
} from "lucide-react";
import { toast } from "sonner";
import {
  CodexIcon,
  ConfluenceIcon,
  Microsoft365Icon,
  OneNoteIcon,
  OneDriveIcon,
  OpenClawIcon,
  OutlookCalendarIcon,
  PlannerIcon,
  TeamsIcon,
  ToDoIcon,
} from "@/components/IntegrationIcons";
import { Button } from "@/components/ui/button";
import {
  teamsDetectionService,
  TeamsDetectionStatus,
} from "@/services/teamsDetectionService";
import { useMicrosoftExport } from "@/hooks/useMicrosoftExport";
import { setPendingCalendar } from "@/lib/meetingCalendar";
import {
  getExportDestinations,
  setExportDestinations,
  type ConfluenceExportMode,
} from "@/lib/exportDestinations";
import {
  confluenceExportService,
  type ConfluenceConnectionStatus,
} from "@/services/confluenceExportService";
import { ONENOTE_LARGE_LIBRARY_MESSAGE } from "@/services/microsoftExportService";
import {
  microsoftExportService,
  type DriveDestination,
} from "@/services/microsoftExportService";
import {
  getTeamsDetectionMode,
  setTeamsDetectionMode,
  type TeamsDetectionMode,
} from "@/lib/autoRecord";

type AddonState =
  | "ready"
  | "prompt"
  | "planned"
  | "provider"
  | "advanced"
  | "connected"
  | "connecting"
  | "signin";

function stateBadge(state: AddonState) {
  switch (state) {
    case "ready":
    case "connected":
      return "Ready";
    case "connecting":
      return "Connecting…";
    case "signin":
      return "Sign-in required";
    case "prompt":
      return "Prompt only";
    case "provider":
      return "Provider";
    case "advanced":
      return "Advanced";
    case "planned":
    default:
      return "Not implemented";
  }
}

function stateClasses(state: AddonState) {
  switch (state) {
    // Solid saturated pills with white text: readable in both light and dark
    // mode, and immune to the globals.css `.dark .bg-*-100` overrides that
    // otherwise hijack soft-tint utilities in dark mode (unreadable text).
    case "ready":
    case "connected":
      return "border-transparent bg-emerald-600 text-white";
    case "connecting":
    case "signin":
      return "border-transparent bg-primary text-white";
    case "prompt":
      return "border-transparent bg-primary text-white";
    case "provider":
      return "border-primary/30 bg-primary/15 text-primary";
    case "advanced":
      return "border-transparent bg-amber-600 text-white";
    case "planned":
    default:
      return "border-border bg-muted text-foreground";
  }
}

interface AddonPanelProps {
  icon: ElementType;
  title: string;
  state: AddonState;
  detail: string;
  children?: ReactNode;
  /** Override the default state-derived badge text (the chip still uses the
   *  state's color classes unless `badgeClasses` is also given). */
  badgeLabel?: string;
  badgeClasses?: string;
  showBadge?: boolean;
}

function AddonPanel({
  icon: Icon,
  title,
  state,
  detail,
  children,
  badgeLabel,
  badgeClasses,
  showBadge = true,
}: AddonPanelProps) {
  return (
    <section className="rounded-lg border border-border bg-card p-5 shadow-sm">
      <div className="flex items-start justify-between gap-4">
        <div className="flex min-w-0 items-start gap-3">
          <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg border border-border bg-background text-primary shadow-sm">
            <Icon className="h-5 w-5" />
          </div>
          <div className="min-w-0">
            <h3 className="text-base font-semibold text-foreground">{title}</h3>
            <p className="mt-1 text-sm text-muted-foreground">{detail}</p>
          </div>
        </div>
        {showBadge && (
          <span
            className={`shrink-0 rounded-full border px-2.5 py-1 text-xs font-medium ${badgeClasses ?? stateClasses(state)}`}
          >
            {badgeLabel ?? stateBadge(state)}
          </span>
        )}
      </div>
      {children && <div className="mt-4">{children}</div>}
    </section>
  );
}

function DetectionSummary({ status }: { status: TeamsDetectionStatus | null }) {
  if (!status) {
    return (
      <p className="text-sm text-muted-foreground">
        Status has not been checked in this session.
      </p>
    );
  }

  return (
    <div className="grid gap-3 text-sm sm:grid-cols-2 lg:grid-cols-4">
      <div className="rounded-lg border border-border bg-muted p-3">
        <p className="text-xs text-muted-foreground">Platform</p>
        <p className="font-medium text-foreground">{status.platform}</p>
      </div>
      <div className="rounded-lg border border-border bg-muted p-3">
        <p className="text-xs text-muted-foreground">Status</p>
        <p className="font-medium text-foreground">{status.status}</p>
      </div>
      <div className="rounded-lg border border-border bg-muted p-3">
        <p className="text-xs text-muted-foreground">Confidence</p>
        <p className="font-medium text-foreground">
          {Math.round(status.confidence * 100)}%
        </p>
      </div>
      <div className="rounded-lg border border-border bg-muted p-3">
        <p className="text-xs text-muted-foreground">Action</p>
        <p className="font-medium text-foreground">
          {status.nextRecommendedAction}
        </p>
      </div>

      {/* Diagnostics so detection misses are debuggable in the shipped app. */}
      <div className="sm:col-span-2 lg:col-span-4 space-y-3 rounded-lg border border-border bg-background p-3">
        <p className="text-xs font-medium text-muted-foreground">
          Signals (need a meeting window title to detect a call)
        </p>
        <ul className="space-y-1 text-xs">
          {status.signals.map((s) => (
            <li key={s.detector} className="flex items-start gap-2">
              <span className={s.matched ? "text-emerald-500" : "text-muted-foreground"}>
                {s.matched ? "✓" : "✗"}
              </span>
              <span className="text-foreground">
                <span className="font-medium">{s.detector}</span>
                <span className="text-muted-foreground"> — {s.detail}</span>
              </span>
            </li>
          ))}
        </ul>
        <p className="text-xs text-muted-foreground">
          Teams processes: {status.diagnostics.teamsProcessCount} · Browser:{" "}
          {status.diagnostics.browserProcessCount} · Windows scanned:{" "}
          {status.diagnostics.relevantWindowCount} · Meeting titles:{" "}
          {status.diagnostics.meetingTitleCount}
        </p>
        {status.candidates.length > 0 && (
          <div>
            <p className="text-xs font-medium text-muted-foreground">Windows detected</p>
            <ul className="mt-1 space-y-0.5 text-xs text-foreground">
              {status.candidates.map((c, i) => (
                <li key={i} className="truncate">
                  <span className="text-muted-foreground">[{c.source}]</span>{" "}
                  {c.windowTitle ?? c.processName ?? "(no title)"}
                </li>
              ))}
            </ul>
          </div>
        )}
      </div>
    </div>
  );
}

function MicrosoftSignInPanel() {
  const ms = useMicrosoftExport();

  const panelState: AddonState = useMemo(() => {
    if (ms.connection.state === "connected") return "connected";
    if (ms.connection.state === "connecting" || ms.signingIn)
      return "connecting";
    return "signin";
  }, [ms.connection.state, ms.signingIn]);

  const detail = useMemo(() => {
    if (ms.connection.state === "connected") {
      return `Signed in as ${ms.connection.userDisplayName ?? ms.connection.userEmail ?? "Microsoft user"}. OneNote, OneDrive, Planner, and To Do exports are available.`;
    }
    if (ms.connection.state === "connecting" || ms.signingIn) {
      return "Waiting for Microsoft sign-in to complete…";
    }
    if (ms.connection.state === "expired") {
      return "Microsoft session expired. Sign in again to re-enable exports.";
    }
    return "Sign in with your Microsoft account to enable OneNote, OneDrive, Planner, and To Do exports.";
  }, [ms.connection, ms.signingIn]);

  return (
    <AddonPanel
      icon={Microsoft365Icon}
      title="Microsoft account"
      state={panelState}
      detail={detail}
    >
      <div className="space-y-3">
        {ms.connection.state === "connected" && (
          <>
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2 text-sm text-muted-foreground">
                <CheckCircle2 className="h-4 w-4 text-emerald-600" />
                <span>
                  {ms.connection.userDisplayName}
                  {ms.connection.userEmail && (
                    <span className="ml-1 text-xs opacity-70">
                      ({ms.connection.userEmail})
                    </span>
                  )}
                </span>
              </div>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={ms.signOut}
              >
                <LogOut className="mr-2 h-4 w-4" />
                Sign out
              </Button>
            </div>
            {ms.connection.grantedScopes !== undefined &&
              ms.connection.grantedScopes !== null &&
              !/\bNotes\./i.test(ms.connection.grantedScopes ?? "") && (
                <div className="flex items-start gap-2 rounded-lg border border-amber-300 bg-amber-50 p-3 text-xs text-amber-800 dark:border-amber-900 dark:bg-amber-950/40 dark:text-amber-200">
                  <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
                  <span>
                    This session was granted no OneNote permission, so notebook
                    discovery will fail. Granted scopes:{" "}
                    <code className="break-all">
                      {ms.connection.grantedScopes || "(none)"}
                    </code>
                    . Sign out and sign in again to grant Microsoft export
                    access; if the consent screen does not list them, the Entra
                    app registration needs those Graph permissions and admin
                    consent.
                  </span>
                </div>
              )}
            {ms.connection.grantedScopes !== undefined &&
              ms.connection.grantedScopes !== null &&
              !/\bFiles\.ReadWrite\b/i.test(ms.connection.grantedScopes ?? "") && (
                <div className="flex items-start gap-2 rounded-lg border border-amber-300 bg-amber-50 p-3 text-xs text-amber-800 dark:border-amber-900 dark:bg-amber-950/40 dark:text-amber-200">
                  <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
                  <span>
                    This session was granted no OneDrive file permission. Sign
                    out and sign in again to grant DOCX/PDF export access.
                  </span>
                </div>
              )}
          </>
        )}

        {(ms.connection.state === "connecting" || ms.signingIn) && (
          <div className="rounded-lg border border-primary bg-primary/10 p-4 dark:border-blue-900 dark:bg-primary/40">
            <div className="flex items-center gap-2 text-sm text-primary dark:text-primary">
              <Loader2 className="h-4 w-4 animate-spin" />
              <span>
                Complete sign-in in your browser, then return to ClawScribe.
              </span>
            </div>
          </div>
        )}

        {ms.connection.state !== "connected" &&
          ms.connection.state !== "connecting" &&
          !ms.signingIn && (
            <Button type="button" variant="outline" onClick={ms.signIn}>
              <LogIn className="mr-2 h-4 w-4" />
              Sign in with Microsoft
            </Button>
          )}

        {ms.error && (
          <div className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
            <span>{ms.error}</span>
          </div>
        )}
      </div>
    </AddonPanel>
  );
}

// OneNote notebook names reject ? * \ / : < > | ' # and must be <= 128 chars.
// Mirror the backend sanitizer so the field shows what will actually be created.
const NOTEBOOK_FORBIDDEN = /[?*\\/:<>|'#]/g;
function sanitizeNotebookName(raw: string): string {
  return raw.replace(NOTEBOOK_FORBIDDEN, " ").replace(/\s+/g, " ").trimStart().slice(0, 128);
}

function sanitizeToDoListName(raw: string): string {
  return raw.replace(/\s+/g, " ").trimStart().slice(0, 255);
}

function sanitizeOneDriveFolderName(raw: string): string {
  return raw
    .replace(/["*:<>?/\\|]/g, " ")
    .replace(/\s+/g, " ")
    .trimStart()
    .slice(0, 120);
}

const NEW_OPTION = "__new__";

function OneNotePanel() {
  const ms = useMicrosoftExport();
  const saved = getExportDestinations();
  const [selectedNotebook, setSelectedNotebook] = useState<string>(
    saved.notebookId ?? "",
  );
  const [creatingNotebook, setCreatingNotebook] = useState(false);
  const [newNotebookName, setNewNotebookName] = useState("");
  const [savingNotebook, setSavingNotebook] = useState(false);

  const submitNewNotebook = async () => {
    const name = sanitizeNotebookName(newNotebookName).trim();
    if (!name) return;
    setSavingNotebook(true);
    const nb = await ms.createNotebook(name);
    setSavingNotebook(false);
    if (nb) {
      setSelectedNotebook(nb.id);
      setCreatingNotebook(false);
      setNewNotebookName("");
    }
  };

  const isConnected = ms.connection.state === "connected";

  useEffect(() => {
    if (isConnected && ms.notebooks.length === 0 && !ms.loadingNotebooks) {
      void ms.loadNotebooks();
    }
  }, [isConnected]);

  // Persist the notebook destination. Exports create a fresh dated section in
  // this notebook, so Settings never lists or stores section ids.
  useEffect(() => {
    if (!selectedNotebook) return;
    const notebookName = ms.notebooks.find((n) => n.id === selectedNotebook)
      ?.displayName;
    setExportDestinations({
      notebookId: selectedNotebook,
      notebookName,
      sectionId: undefined,
      sectionName: undefined,
    });
  }, [selectedNotebook, ms.notebooks]);

  const panelState: AddonState = isConnected ? "connected" : "signin";
  const detail = isConnected
    ? "Pick the notebook where exports create a fresh dated section for each meeting."
    : "Sign in with Microsoft above to enable OneNote export.";
  const selectedNotebookMissing =
    !!selectedNotebook && !ms.notebooks.some((nb) => nb.id === selectedNotebook);

  return (
    <AddonPanel
      icon={OneNoteIcon}
      title="OneNote export"
      state={panelState}
      detail={detail}
      showBadge={!isConnected}
    >
      {isConnected && (
        <div className="space-y-3">
          <div>
            <div className="mb-1 flex items-center justify-between">
              <label className="block text-xs font-medium text-muted-foreground">
                Notebook
              </label>
              <button
                type="button"
                className="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground disabled:opacity-50"
                onClick={() => void ms.loadNotebooks()}
                disabled={ms.loadingNotebooks}
              >
                <RefreshCw
                  className={`h-3 w-3 ${ms.loadingNotebooks ? "animate-spin" : ""}`}
                />
                Reload
              </button>
            </div>
            <select
              className="w-full rounded-lg border border-border bg-muted px-3 py-2 text-sm"
              value={creatingNotebook ? NEW_OPTION : selectedNotebook}
              onChange={(e) => {
                if (e.target.value === NEW_OPTION) {
                  setCreatingNotebook(true);
                } else {
                  setCreatingNotebook(false);
                  setSelectedNotebook(e.target.value);
                }
              }}
              disabled={ms.loadingNotebooks}
            >
              <option value="">
                {ms.loadingNotebooks ? "Loading…" : "Select a notebook"}
              </option>
              {ms.notebooks.map((nb) => (
                <option key={nb.id} value={nb.id}>
                  {nb.displayName}
                </option>
              ))}
              {selectedNotebookMissing && (
                <option value={selectedNotebook}>
                  {saved.notebookName ?? "Saved notebook"}
                </option>
              )}
              <option value={NEW_OPTION}>+ New notebook…</option>
            </select>
          </div>

          {ms.oneNoteNotebookListingLimited && (
            <p className="rounded-lg border border-amber-500/30 bg-amber-500/10 p-3 text-sm text-amber-800 dark:text-amber-200">
              {ONENOTE_LARGE_LIBRARY_MESSAGE}
            </p>
          )}

          {creatingNotebook && (
            <div className="space-y-2 rounded-lg border border-border bg-muted p-3">
              <label className="block text-xs font-medium text-muted-foreground">
                New notebook name
              </label>
              <div className="flex items-center gap-2">
                <input
                  type="text"
                  autoFocus
                  value={newNotebookName}
                  onChange={(e) =>
                    setNewNotebookName(sanitizeNotebookName(e.target.value))
                  }
                  onKeyDown={(e) => {
                    if (e.key === "Enter") void submitNewNotebook();
                    if (e.key === "Escape") setCreatingNotebook(false);
                  }}
                  placeholder="e.g. Meeting Notes"
                  maxLength={128}
                  className="flex-1 rounded-lg border border-border bg-background px-3 py-2 text-sm"
                />
                <Button
                  type="button"
                  size="sm"
                  onClick={() => void submitNewNotebook()}
                  disabled={savingNotebook || !newNotebookName.trim()}
                >
                  {savingNotebook ? (
                    <Loader2 className="h-4 w-4 animate-spin" />
                  ) : (
                    "Create"
                  )}
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={() => setCreatingNotebook(false)}
                  disabled={savingNotebook}
                >
                  Cancel
                </Button>
              </div>
              <p className="text-xs text-muted-foreground">
                Can&apos;t contain {"? * \\ / : < > | ' #"} — those are removed
                automatically. Max 128 characters.
              </p>
            </div>
          )}

          {ms.error && (
            <div className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
              <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
              <span>{ms.error}</span>
            </div>
          )}
          {!ms.loadingNotebooks && !ms.error && ms.notebooks.length === 0 && (
            <p className="rounded-lg border border-border bg-muted p-3 text-sm text-muted-foreground">
              No OneNote notebooks were returned for this account. If you expect
              notebooks here, confirm you signed in with the same account that
              owns them and that this app has the OneNote permission, then use
              Reload.
            </p>
          )}
          {selectedNotebook && (
            <div className="flex items-center gap-2 text-sm text-emerald-700 dark:text-emerald-300">
              <CheckCircle2 className="h-4 w-4" />
              <span>
                OneNote notebook ready. Each export creates a new dated section.
              </span>
            </div>
          )}
        </div>
      )}
    </AddonPanel>
  );
}

function PlannerPanel() {
  const ms = useMicrosoftExport();
  const saved = getExportDestinations();
  const [selectedPlan, setSelectedPlan] = useState<string>(saved.planId ?? "");
  const [selectedBucket, setSelectedBucket] = useState<string>(
    saved.bucketId ?? "",
  );
  const [creatingBucket, setCreatingBucket] = useState(false);
  const [newBucketName, setNewBucketName] = useState("");
  const [savingBucket, setSavingBucket] = useState(false);
  const [aiPolish, setAiPolish] = useState<boolean>(saved.plannerAiPolish ?? false);

  const toggleAiPolish = () => {
    const next = !aiPolish;
    setAiPolish(next);
    setExportDestinations({ plannerAiPolish: next });
  };

  const submitNewBucket = async () => {
    const name = newBucketName.replace(/\s+/g, " ").trim();
    if (!name || !selectedPlan) return;
    setSavingBucket(true);
    const bucket = await ms.createBucket(selectedPlan, name);
    setSavingBucket(false);
    if (bucket) {
      setSelectedBucket(bucket.id);
      setCreatingBucket(false);
      setNewBucketName("");
    }
  };

  const isConnected = ms.connection.state === "connected";

  useEffect(() => {
    if (isConnected && ms.plans.length === 0 && !ms.loadingPlans) {
      void ms.loadPlans();
    }
  }, [isConnected]);

  useEffect(() => {
    if (selectedPlan) {
      void ms.loadBuckets(selectedPlan);
    }
  }, [selectedPlan]);

  useEffect(() => {
    if (!selectedBucket) return;
    setExportDestinations({
      planId: selectedPlan,
      planName: ms.plans.find((p) => p.id === selectedPlan)?.title,
      bucketId: selectedBucket,
      bucketName: ms.buckets.find((b) => b.id === selectedBucket)?.name,
    });
  }, [selectedBucket, selectedPlan, ms.plans, ms.buckets]);

  const panelState: AddonState = isConnected ? "connected" : "signin";
  const detail = isConnected
    ? "Pick a plan and a default bucket. You can review tasks and re-route each one per export."
    : "Sign in with Microsoft above to enable Planner export.";

  return (
    <AddonPanel
      icon={PlannerIcon}
      title="Planner task export"
      state={panelState}
      detail={detail}
      showBadge={!isConnected}
    >
      {isConnected && (
        <div className="space-y-3">
          <div className="grid gap-3 sm:grid-cols-2">
            <div>
              <div className="mb-1 flex items-center justify-between">
                <label className="block text-xs font-medium text-muted-foreground">
                  Plan
                </label>
                <button
                  type="button"
                  className="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground disabled:opacity-50"
                  onClick={() => void ms.loadPlans()}
                  disabled={ms.loadingPlans}
                >
                  <RefreshCw
                    className={`h-3 w-3 ${ms.loadingPlans ? "animate-spin" : ""}`}
                  />
                  Reload
                </button>
              </div>
              <select
                className="w-full rounded-lg border border-border bg-muted px-3 py-2 text-sm"
                value={selectedPlan}
                onChange={(e) => {
                  setSelectedPlan(e.target.value);
                  setSelectedBucket("");
                }}
                disabled={ms.loadingPlans}
              >
                <option value="">
                  {ms.loadingPlans ? "Loading…" : "Select a plan"}
                </option>
                {ms.plans.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.title}
                  </option>
                ))}
              </select>
            </div>
            <div>
              <label className="mb-1 block text-xs font-medium text-muted-foreground">
                Default Bucket
              </label>
              <select
                className="w-full rounded-lg border border-border bg-muted px-3 py-2 text-sm"
                value={creatingBucket ? NEW_OPTION : selectedBucket}
                onChange={(e) => {
                  if (e.target.value === NEW_OPTION) {
                    setCreatingBucket(true);
                  } else {
                    setCreatingBucket(false);
                    setSelectedBucket(e.target.value);
                  }
                }}
                disabled={!selectedPlan || ms.loadingBuckets}
              >
                <option value="">
                  {ms.loadingBuckets
                    ? "Loading…"
                    : !selectedPlan
                      ? "Select a plan first"
                      : "Select a bucket"}
                </option>
                {ms.buckets.map((b) => (
                  <option key={b.id} value={b.id}>
                    {b.name}
                  </option>
                ))}
                {selectedPlan && (
                  <option value={NEW_OPTION}>+ New bucket…</option>
                )}
              </select>
            </div>
          </div>

          {creatingBucket && (
            <div className="space-y-2 rounded-lg border border-border bg-muted p-3">
              <label className="block text-xs font-medium text-muted-foreground">
                New bucket name
              </label>
              <div className="flex items-center gap-2">
                <input
                  type="text"
                  autoFocus
                  value={newBucketName}
                  onChange={(e) => setNewBucketName(e.target.value.slice(0, 255))}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") void submitNewBucket();
                    if (e.key === "Escape") setCreatingBucket(false);
                  }}
                  placeholder="e.g. Action items"
                  maxLength={255}
                  className="flex-1 rounded-lg border border-border bg-background px-3 py-2 text-sm"
                />
                <Button
                  type="button"
                  size="sm"
                  onClick={() => void submitNewBucket()}
                  disabled={savingBucket || !newBucketName.trim()}
                >
                  {savingBucket ? (
                    <Loader2 className="h-4 w-4 animate-spin" />
                  ) : (
                    "Create"
                  )}
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={() => setCreatingBucket(false)}
                  disabled={savingBucket}
                >
                  Cancel
                </Button>
              </div>
            </div>
          )}
          <label className="flex cursor-pointer items-start justify-between gap-4 rounded-lg border border-border bg-muted p-3">
            <span>
              <span className="block text-sm font-medium text-foreground">
                AI-generate task titles &amp; notes
              </span>
              <span className="mt-0.5 block text-xs text-muted-foreground">
                Rewrite action items into clean titles and notes with your summary
                model. You review and edit them in the export preview before anything
                is created.
              </span>
            </span>
            <button
              type="button"
              role="switch"
              aria-checked={aiPolish}
              onClick={toggleAiPolish}
              className={`relative mt-0.5 inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors ${
                aiPolish ? "bg-primary" : "bg-border"
              }`}
            >
              <span
                className={`inline-block h-5 w-5 transform rounded-full bg-background shadow transition-transform ${
                  aiPolish ? "translate-x-5" : "translate-x-0.5"
                }`}
              />
            </button>
          </label>

          {ms.error && (
            <div className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
              <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
              <span>{ms.error}</span>
            </div>
          )}
          {!ms.loadingPlans && !ms.error && ms.plans.length === 0 && (
            <p className="rounded-lg border border-border bg-muted p-3 text-sm text-muted-foreground">
              No Planner plans found for this account. Create a plan in Planner
              or Microsoft Teams, then use Reload to pick it here. (Plans can&apos;t
              be created from ClawScribe — they must belong to a Microsoft 365
              group.)
            </p>
          )}
          {selectedBucket && (
            <div className="flex items-center gap-2 text-sm text-emerald-700 dark:text-emerald-300">
              <CheckCircle2 className="h-4 w-4" />
              <span>
                Planner destination ready. Export from a meeting&apos;s summary
                panel.
              </span>
            </div>
          )}
        </div>
      )}
    </AddonPanel>
  );
}

function ToDoPanel() {
  const ms = useMicrosoftExport();
  const saved = getExportDestinations();
  const [selectedList, setSelectedList] = useState<string>(saved.todoListId ?? "");
  const [creatingList, setCreatingList] = useState(false);
  const [newListName, setNewListName] = useState("");
  const [savingList, setSavingList] = useState(false);
  const savingListRef = useRef(false);
  const isConnected = ms.connection.state === "connected";

  const submitNewList = async () => {
    if (savingListRef.current) return;
    const name = sanitizeToDoListName(newListName).trim();
    if (!name) return;
    savingListRef.current = true;
    setSavingList(true);
    try {
      const list = await ms.createToDoList(name);
      if (list) {
        setSelectedList(list.id);
        setCreatingList(false);
        setNewListName("");
      }
    } finally {
      savingListRef.current = false;
      setSavingList(false);
    }
  };

  useEffect(() => {
    if (isConnected && ms.todoLists.length === 0 && !ms.loadingToDoLists) {
      void ms.loadToDoLists();
    }
  }, [isConnected]);

  useEffect(() => {
    if (!selectedList) return;
    setExportDestinations({
      todoListId: selectedList,
      todoListName: ms.todoLists.find((l) => l.id === selectedList)?.displayName,
    });
  }, [selectedList, ms.todoLists]);

  const panelState: AddonState = isConnected ? "connected" : "signin";
  const detail = isConnected
    ? "Pick the personal To Do list for reviewed meeting action items."
    : "Sign in with Microsoft above to enable To Do export.";
  const selectedListMissing =
    !!selectedList && !ms.todoLists.some((l) => l.id === selectedList);

  return (
    <AddonPanel
      icon={ToDoIcon}
      title="Microsoft To Do export"
      state={panelState}
      detail={detail}
      showBadge={!isConnected}
    >
      {isConnected && (
        <div className="space-y-3">
          <div>
            <div className="mb-1 flex items-center justify-between">
              <label className="block text-xs font-medium text-muted-foreground">
                To Do list
              </label>
              <button
                type="button"
                className="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground disabled:opacity-50"
                onClick={() => void ms.loadToDoLists()}
                disabled={ms.loadingToDoLists}
              >
                <RefreshCw
                  className={`h-3 w-3 ${ms.loadingToDoLists ? "animate-spin" : ""}`}
                />
                Reload
              </button>
            </div>
            <select
              className="w-full rounded-lg border border-border bg-muted px-3 py-2 text-sm"
              value={creatingList ? NEW_OPTION : selectedList}
              onChange={(e) => {
                if (e.target.value === NEW_OPTION) {
                  setCreatingList(true);
                } else {
                  setCreatingList(false);
                  setSelectedList(e.target.value);
                }
              }}
              disabled={ms.loadingToDoLists}
            >
              <option value="">
                {ms.loadingToDoLists ? "Loading…" : "Select a To Do list"}
              </option>
              {ms.todoLists.map((list) => (
                <option key={list.id} value={list.id}>
                  {list.displayName}
                </option>
              ))}
              {selectedListMissing && (
                <option value={selectedList}>
                  {saved.todoListName ?? "Saved To Do list"}
                </option>
              )}
              <option value={NEW_OPTION}>+ New To Do list…</option>
            </select>
          </div>

          {creatingList && (
            <div className="space-y-2 rounded-lg border border-border bg-muted p-3">
              <label className="block text-xs font-medium text-muted-foreground">
                New To Do list name
              </label>
              <div className="flex items-center gap-2">
                <input
                  type="text"
                  autoFocus
                  value={newListName}
                  onChange={(e) =>
                    setNewListName(sanitizeToDoListName(e.target.value))
                  }
                  onKeyDown={(e) => {
                    if (e.key === "Enter") void submitNewList();
                    if (e.key === "Escape") setCreatingList(false);
                  }}
                  placeholder="e.g. Meeting action items"
                  maxLength={255}
                  className="flex-1 rounded-lg border border-border bg-background px-3 py-2 text-sm"
                />
                <Button
                  type="button"
                  size="sm"
                  onClick={() => void submitNewList()}
                  disabled={savingList || !newListName.trim()}
                >
                  {savingList ? (
                    <Loader2 className="h-4 w-4 animate-spin" />
                  ) : (
                    "Create"
                  )}
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={() => setCreatingList(false)}
                  disabled={savingList}
                >
                  Cancel
                </Button>
              </div>
              <p className="text-xs text-muted-foreground">
                Creates a personal Microsoft To Do list in the signed-in
                account, then saves it as the export destination.
              </p>
            </div>
          )}

          {ms.error && (
            <div className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
              <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
              <span>{ms.error}</span>
            </div>
          )}
          {!ms.loadingToDoLists && !ms.error && ms.todoLists.length === 0 && (
            <p className="rounded-lg border border-border bg-muted p-3 text-sm text-muted-foreground">
              No Microsoft To Do lists were returned for this account. Create
              one here, or use Reload after creating one elsewhere.
            </p>
          )}
          {selectedList && (
            <div className="flex items-center gap-2 text-sm text-emerald-700 dark:text-emerald-300">
              <CheckCircle2 className="h-4 w-4" />
              <span>To Do destination ready. Export reviewed action items from a meeting summary.</span>
            </div>
          )}
        </div>
      )}
    </AddonPanel>
  );
}

function OneDrivePanel() {
  const ms = useMicrosoftExport();
  const saved = getExportDestinations();
  const [rootDestination, setRootDestination] = useState<DriveDestination | null>(null);
  const [selectedDestination, setSelectedDestination] = useState<DriveDestination | null>(
    saved.oneDriveDestination ?? null,
  );
  const [loadingRoot, setLoadingRoot] = useState(false);
  const [sharingUrl, setSharingUrl] = useState("");
  const [resolvingUrl, setResolvingUrl] = useState(false);
  const [newFolderName, setNewFolderName] = useState("");
  const [creatingFolder, setCreatingFolder] = useState(false);
  const [includePdf, setIncludePdf] = useState(saved.oneDriveIncludePdf ?? true);
  const [createOrgLink, setCreateOrgLink] = useState(
    saved.oneDriveCreateOrganizationLink ?? false,
  );
  const [localError, setLocalError] = useState<string | null>(null);
  const isConnected = ms.connection.state === "connected";
  const activeDestination = selectedDestination ?? rootDestination;

  const persistDestination = useCallback((destination: DriveDestination) => {
    setSelectedDestination(destination);
    setExportDestinations({ oneDriveDestination: destination });
  }, []);

  const loadRoot = useCallback(async () => {
    if (!isConnected) return null;
    setLoadingRoot(true);
    setLocalError(null);
    try {
      const destinations = await microsoftExportService.listOneDriveDestinations();
      const root = destinations[0] ?? null;
      setRootDestination(root);
      return root;
    } catch (e) {
      setLocalError(e instanceof Error ? e.message : String(e));
      return null;
    } finally {
      setLoadingRoot(false);
    }
  }, [isConnected]);

  useEffect(() => {
    if (isConnected && !rootDestination && !loadingRoot) {
      void loadRoot();
    }
  }, [isConnected, loadRoot, loadingRoot, rootDestination]);

  useEffect(() => {
    setExportDestinations({
      oneDriveIncludePdf: includePdf,
      oneDriveCreateOrganizationLink: createOrgLink,
    });
  }, [createOrgLink, includePdf]);

  const useRoot = async () => {
    const root = rootDestination ?? (await loadRoot());
    if (root) persistDestination(root);
  };

  const resolveSharingUrl = async () => {
    const url = sharingUrl.trim();
    if (!url) return;
    setResolvingUrl(true);
    setLocalError(null);
    try {
      const destination = await microsoftExportService.resolveOneDriveDestinationUrl(url);
      persistDestination(destination);
      setSharingUrl("");
    } catch (e) {
      setLocalError(e instanceof Error ? e.message : String(e));
    } finally {
      setResolvingUrl(false);
    }
  };

  const createFolder = async () => {
    const name = sanitizeOneDriveFolderName(newFolderName).trim();
    if (!name) return;
    const parent = activeDestination ?? (await loadRoot());
    if (!parent) return;

    setCreatingFolder(true);
    setLocalError(null);
    try {
      const destination = await microsoftExportService.createOneDriveDestinationFolder(
        parent,
        name,
      );
      persistDestination(destination);
      setNewFolderName("");
    } catch (e) {
      setLocalError(e instanceof Error ? e.message : String(e));
    } finally {
      setCreatingFolder(false);
    }
  };

  const panelState: AddonState = isConnected ? "connected" : "signin";
  const detail = isConnected
    ? "Export full meeting notes as DOCX and PDF files to OneDrive or SharePoint."
    : "Sign in with Microsoft above to enable OneDrive file export.";

  return (
    <AddonPanel
      icon={OneDriveIcon}
      title="OneDrive file export"
      state={panelState}
      detail={detail}
      showBadge={!isConnected}
    >
      {isConnected && (
        <div className="space-y-3">
          <div className="rounded-lg border border-border bg-muted p-3">
            <div className="flex items-start justify-between gap-3">
              <div className="min-w-0">
                <p className="text-xs font-medium text-muted-foreground">Destination folder</p>
                <p className="mt-1 truncate text-sm font-medium text-foreground">
                  {activeDestination?.name ?? "Loading OneDrive root..."}
                </p>
                {activeDestination?.webUrl && (
                  <p className="mt-0.5 truncate text-xs text-muted-foreground">
                    {activeDestination.webUrl}
                  </p>
                )}
              </div>
              <Button
                type="button"
                size="sm"
                variant="outline"
                onClick={() => void useRoot()}
                disabled={loadingRoot}
              >
                {loadingRoot ? <Loader2 className="h-4 w-4 animate-spin" /> : "Use root"}
              </Button>
            </div>
          </div>

          <div className="grid gap-3 md:grid-cols-[1fr_auto]">
            <input
              type="url"
              value={sharingUrl}
              onChange={(e) => setSharingUrl(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void resolveSharingUrl();
              }}
              placeholder="Paste OneDrive or SharePoint folder link"
              className="min-w-0 rounded-lg border border-border bg-background px-3 py-2 text-sm"
              disabled={resolvingUrl}
            />
            <Button
              type="button"
              variant="outline"
              onClick={() => void resolveSharingUrl()}
              disabled={resolvingUrl || !sharingUrl.trim()}
            >
              {resolvingUrl ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
              Resolve folder
            </Button>
          </div>

          <div className="grid gap-3 md:grid-cols-[1fr_auto]">
            <input
              type="text"
              value={newFolderName}
              onChange={(e) => setNewFolderName(sanitizeOneDriveFolderName(e.target.value))}
              onKeyDown={(e) => {
                if (e.key === "Enter") void createFolder();
              }}
              placeholder="Create subfolder, e.g. ClawScribe"
              maxLength={120}
              className="min-w-0 rounded-lg border border-border bg-background px-3 py-2 text-sm"
              disabled={creatingFolder}
            />
            <Button
              type="button"
              variant="outline"
              onClick={() => void createFolder()}
              disabled={creatingFolder || !newFolderName.trim()}
            >
              {creatingFolder ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
              Create folder
            </Button>
          </div>

          <div className="grid gap-3 sm:grid-cols-2">
            <label className="flex cursor-pointer items-start justify-between gap-4 rounded-lg border border-border bg-muted p-3">
              <span>
                <span className="block text-sm font-medium text-foreground">Include PDF</span>
                <span className="mt-0.5 block text-xs text-muted-foreground">
                  Upload a PDF copy beside the DOCX.
                </span>
              </span>
              <button
                type="button"
                role="switch"
                aria-checked={includePdf}
                onClick={() => setIncludePdf((value) => !value)}
                className={`relative mt-0.5 inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors ${
                  includePdf ? "bg-primary" : "bg-border"
                }`}
              >
                <span
                  className={`inline-block h-5 w-5 transform rounded-full bg-background shadow transition-transform ${
                    includePdf ? "translate-x-5" : "translate-x-0.5"
                  }`}
                />
              </button>
            </label>

            <label className="flex cursor-pointer items-start justify-between gap-4 rounded-lg border border-border bg-muted p-3">
              <span>
                <span className="block text-sm font-medium text-foreground">Create org links</span>
                <span className="mt-0.5 block text-xs text-muted-foreground">
                  Ask Graph for view links scoped to your organization.
                </span>
              </span>
              <button
                type="button"
                role="switch"
                aria-checked={createOrgLink}
                onClick={() => setCreateOrgLink((value) => !value)}
                className={`relative mt-0.5 inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors ${
                  createOrgLink ? "bg-primary" : "bg-border"
                }`}
              >
                <span
                  className={`inline-block h-5 w-5 transform rounded-full bg-background shadow transition-transform ${
                    createOrgLink ? "translate-x-5" : "translate-x-0.5"
                  }`}
                />
              </button>
            </label>
          </div>

          {(localError || ms.error) && (
            <div className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
              <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
              <span>{localError ?? ms.error}</span>
            </div>
          )}

          {activeDestination && (
            <div className="flex items-center gap-2 text-sm text-emerald-700 dark:text-emerald-300">
              <CheckCircle2 className="h-4 w-4" />
              <span>
                File destination ready. Exports upload a DOCX{includePdf ? " and PDF" : ""}.
              </span>
            </div>
          )}
        </div>
      )}
    </AddonPanel>
  );
}

function ConfluencePanel() {
  const saved = getExportDestinations();
  const [mode, setMode] = useState<ConfluenceExportMode>(
    saved.confluenceMode ?? "draft",
  );
  const [createUrl, setCreateUrl] = useState(saved.confluenceCreateUrl ?? "");
  const [openAfterCopy, setOpenAfterCopy] = useState(
    saved.confluenceOpenAfterCopy ?? true,
  );
  const [baseUrl, setBaseUrl] = useState(saved.confluenceBaseUrl ?? "");
  const [spaceKey, setSpaceKey] = useState(saved.confluenceSpaceKey ?? "");
  const [parentId, setParentId] = useState(saved.confluenceParentId ?? "");
  const [patInput, setPatInput] = useState("");
  const [status, setStatus] = useState<ConfluenceConnectionStatus | null>(null);
  const [busy, setBusy] = useState<"save" | "clear" | "test" | null>(null);
  const trimmedUrl = createUrl.trim();
  const trimmedBaseUrl = baseUrl.trim();
  const trimmedSpaceKey = spaceKey.trim();
  const trimmedParentId = parentId.trim();

  useEffect(() => {
    setExportDestinations({
      confluenceMode: mode,
      confluenceCreateUrl: trimmedUrl || undefined,
      confluenceOpenAfterCopy: openAfterCopy,
      confluenceBaseUrl: trimmedBaseUrl || undefined,
      confluenceSpaceKey: trimmedSpaceKey || undefined,
      confluenceParentId: trimmedParentId || undefined,
    });
  }, [
    mode,
    openAfterCopy,
    trimmedBaseUrl,
    trimmedParentId,
    trimmedSpaceKey,
    trimmedUrl,
  ]);

  const testConnection = useCallback(async () => {
    if (!trimmedBaseUrl) {
      setStatus({
        tokenConfigured: false,
        reachable: false,
        userDisplayName: null,
        message: "Enter a Confluence base URL first.",
      });
      return;
    }

    setBusy("test");
    try {
      setStatus(await confluenceExportService.connectionStatus(trimmedBaseUrl));
    } catch (e) {
      setStatus({
        tokenConfigured: true,
        reachable: false,
        userDisplayName: null,
        message: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setBusy(null);
    }
  }, [trimmedBaseUrl]);

  const savePat = useCallback(async () => {
    const pat = patInput.trim();
    if (!pat) {
      setStatus({
        tokenConfigured: false,
        reachable: false,
        userDisplayName: null,
        message: "Paste a Confluence personal access token first.",
      });
      return;
    }

    setBusy("save");
    try {
      await confluenceExportService.savePat(pat);
      setPatInput("");
      if (trimmedBaseUrl) {
        setStatus(await confluenceExportService.connectionStatus(trimmedBaseUrl));
      } else {
        setStatus({
          tokenConfigured: true,
          reachable: false,
          userDisplayName: null,
          message: "PAT saved. Add a base URL, then test the connection.",
        });
      }
    } catch (e) {
      setStatus({
        tokenConfigured: false,
        reachable: false,
        userDisplayName: null,
        message: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setBusy(null);
    }
  }, [patInput, trimmedBaseUrl]);

  const clearPat = useCallback(async () => {
    setBusy("clear");
    try {
      await confluenceExportService.clearPat();
      setPatInput("");
      setStatus({
        tokenConfigured: false,
        reachable: false,
        userDisplayName: null,
        message: "Saved Confluence PAT cleared.",
      });
    } catch (e) {
      setStatus({
        tokenConfigured: true,
        reachable: false,
        userDisplayName: null,
        message: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setBusy(null);
    }
  }, []);

  const restDestinationConfigured = !!trimmedBaseUrl && !!trimmedSpaceKey;
  const restReady = restDestinationConfigured && !!status?.reachable;
  const badgeLabel =
    mode === "draft"
      ? trimmedUrl
        ? "Draft ready"
        : "Copy only"
      : restReady
        ? "Ready"
        : status?.tokenConfigured
          ? "PAT saved"
          : "Needs setup";
  const badgeClasses =
    mode === "draft"
      ? trimmedUrl
        ? "border-transparent bg-emerald-600 text-white"
        : "border-border bg-muted text-foreground"
      : restReady
        ? "border-transparent bg-emerald-600 text-white"
        : "border-transparent bg-amber-600 text-white";

  return (
    <AddonPanel
      icon={ConfluenceIcon}
      title="Confluence export"
      state={mode === "draft" ? "prompt" : restReady ? "ready" : "advanced"}
      badgeLabel={badgeLabel}
      badgeClasses={badgeClasses}
      detail="Export meeting summaries as a browser draft, or create pages directly on self-hosted Confluence with a PAT."
    >
      <div className="space-y-3">
        <div className="grid gap-2 sm:grid-cols-2">
          <button
            type="button"
            onClick={() => setMode("draft")}
            className={`rounded-md border p-3 text-left transition-colors ${
              mode === "draft"
                ? "border-primary bg-primary/10"
                : "border-border bg-muted hover:bg-muted/80"
            }`}
          >
            <span className="block text-sm font-medium text-foreground">
              Browser draft
            </span>
            <span className="mt-1 block text-xs text-muted-foreground">
              Copy rich text and open Confluence in your existing browser
              session. No API token is used.
            </span>
          </button>
          <button
            type="button"
            onClick={() => setMode("rest")}
            className={`rounded-md border p-3 text-left transition-colors ${
              mode === "rest"
                ? "border-primary bg-primary/10"
                : "border-border bg-muted hover:bg-muted/80"
            }`}
          >
            <span className="block text-sm font-medium text-foreground">
              Direct REST
            </span>
            <span className="mt-1 block text-xs text-muted-foreground">
              Create pages through a reachable self-hosted Confluence Server or
              Data Center instance.
            </span>
          </button>
        </div>

        {mode === "draft" ? (
          <>
            <div>
              <label className="mb-1 block text-xs font-medium text-muted-foreground">
                Create page URL
              </label>
              <input
                type="url"
                value={createUrl}
                onChange={(e) => setCreateUrl(e.target.value)}
                placeholder="https://confluence.example.com/confluence/pages/createpage.action?spaceKey=TEAM"
                className="w-full rounded-md border border-border bg-muted px-3 py-2 text-sm text-foreground"
              />
              <p className="mt-1 text-xs text-muted-foreground">
                For Jira/Confluence behind SSO or App Proxy, this opens in your
                browser session. No API token or REST call is used.
              </p>
            </div>

            <label className="flex cursor-pointer items-start justify-between gap-4 rounded-lg border border-border bg-muted p-3">
              <span>
                <span className="block text-sm font-medium text-foreground">
                  Open Confluence after copying
                </span>
                <span className="mt-0.5 block text-xs text-muted-foreground">
                  The summary button copies rich text plus Markdown, then opens
                  the configured create-page URL so you can paste manually.
                </span>
              </span>
              <button
                type="button"
                role="switch"
                aria-checked={openAfterCopy}
                onClick={() => setOpenAfterCopy((v) => !v)}
                className={`relative mt-0.5 inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors ${
                  openAfterCopy ? "bg-primary" : "bg-border"
                }`}
              >
                <span
                  className={`inline-block h-5 w-5 transform rounded-full bg-background shadow transition-transform ${
                    openAfterCopy ? "translate-x-5" : "translate-x-0.5"
                  }`}
                />
              </button>
            </label>
          </>
        ) : (
          <div className="space-y-3">
            <div className="rounded-md border border-amber-500/30 bg-amber-500/10 p-3 text-xs text-muted-foreground">
              <p className="font-medium text-foreground">
                Direct REST requirements
              </p>
              <p className="mt-1">
                Use this for self-hosted Confluence Server or Data Center when
                the instance is reachable from this device, for example through
                your corporate VPN. It requires a Confluence personal access
                token with permission to create pages in the target space.
              </p>
            </div>

            <div className="grid gap-3 sm:grid-cols-2">
              <div className="sm:col-span-2">
                <label className="mb-1 block text-xs font-medium text-muted-foreground">
                  Base URL
                </label>
                <input
                  type="url"
                  value={baseUrl}
                  onChange={(e) => setBaseUrl(e.target.value)}
                  placeholder="https://confluence.example.com/confluence"
                  className="w-full rounded-md border border-border bg-muted px-3 py-2 text-sm text-foreground"
                />
              </div>
              <div>
                <label className="mb-1 block text-xs font-medium text-muted-foreground">
                  Space key
                </label>
                <input
                  value={spaceKey}
                  onChange={(e) => setSpaceKey(e.target.value)}
                  placeholder="TEAM"
                  className="w-full rounded-md border border-border bg-muted px-3 py-2 text-sm text-foreground"
                />
              </div>
              <div>
                <label className="mb-1 block text-xs font-medium text-muted-foreground">
                  Parent page ID optional
                </label>
                <input
                  value={parentId}
                  onChange={(e) => setParentId(e.target.value)}
                  placeholder="123456789"
                  className="w-full rounded-md border border-border bg-muted px-3 py-2 text-sm text-foreground"
                />
              </div>
            </div>

            <div>
              <label className="mb-1 block text-xs font-medium text-muted-foreground">
                Personal access token
              </label>
              <div className="flex flex-col gap-2 sm:flex-row">
                <input
                  type="password"
                  value={patInput}
                  onChange={(e) => setPatInput(e.target.value)}
                  placeholder="Paste PAT to save in OS credentials"
                  className="min-w-0 flex-1 rounded-md border border-border bg-muted px-3 py-2 text-sm text-foreground"
                />
                <Button
                  type="button"
                  variant="outline"
                  onClick={savePat}
                  disabled={busy !== null || !patInput.trim()}
                >
                  {busy === "save" && (
                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  )}
                  Save PAT
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  onClick={clearPat}
                  disabled={busy !== null}
                >
                  {busy === "clear" && (
                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  )}
                  Clear
                </Button>
              </div>
              <p className="mt-1 text-xs text-muted-foreground">
                The token is stored in the OS credential store. It is not saved
                in localStorage or written into exported meeting content.
              </p>
            </div>

            <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
              <Button
                type="button"
                variant="outline"
                onClick={testConnection}
                disabled={busy !== null || !trimmedBaseUrl}
              >
                {busy === "test" && (
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                )}
                Test connection
              </Button>
              <p className="text-xs text-muted-foreground">
                Direct export also needs the base URL and space key above.
              </p>
            </div>

            {status && (
              <div
                className={`flex items-start gap-2 rounded-md border p-3 text-sm ${
                  status.reachable
                    ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300"
                    : "border-border bg-muted text-muted-foreground"
                }`}
              >
                {status.reachable ? (
                  <CheckCircle2 className="mt-0.5 h-4 w-4 shrink-0" />
                ) : (
                  <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
                )}
                <span>{status.message}</span>
              </div>
            )}
          </div>
        )}

        <div className="rounded-lg border border-border bg-muted/50 p-3 text-xs text-muted-foreground">
          <p className="font-medium text-foreground">Export flow</p>
          <p className="mt-1">
            {mode === "draft"
              ? "Meeting summary -> Confluence button -> clipboard -> browser create page. Paste into the editor and save under the space/page you want."
              : "Meeting summary -> Confluence button -> REST API page create. If the API call fails, ClawScribe copies the browser draft instead."}
          </p>
        </div>
      </div>
    </AddonPanel>
  );
}

type OpenClawSubmissionStatus = {
  state: string;
  updated_at: string;
  status_code?: number | null;
  message: string;
};

type OpenClawConfigStatus = {
  enabled: boolean;
  configured: boolean;
  ready: boolean;
  bearer_token_configured: boolean;
  endpoint: string;
  source: string;
  status_message: string;
  config_path: string;
  include_audio_path: boolean;
  last_submission?: OpenClawSubmissionStatus | null;
};

// Live status for the OpenClaw handoff. It's configured as the OpenClaw provider
// under Summary; this is the single place its handoff status is surfaced (it used
// to be duplicated under General).
function OpenClawPanel() {
  const [status, setStatus] = useState<OpenClawConfigStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setStatus(await invoke<OpenClawConfigStatus>("get_openclaw_config_status"));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const panelState: AddonState = status?.ready ? "ready" : "provider";

  return (
    <AddonPanel
      icon={OpenClawIcon}
      title="OpenClaw handoff"
      state={panelState}
      detail="Send finished meetings to your OpenClaw workspace. Configured as the OpenClaw provider under Summary; this shows its live status."
    >
      <div className="space-y-3">
        <div className="flex items-center justify-between gap-3">
          <span className="flex flex-wrap items-center gap-2 text-sm">
            <span
              className={`rounded-full px-2.5 py-1 text-xs font-medium ${
                status?.ready
                  ? "bg-green-100 text-green-800 dark:bg-green-950/40 dark:text-green-200"
                  : "bg-amber-100 text-amber-800 dark:bg-amber-950/40 dark:text-amber-200"
              }`}
            >
              {status?.ready ? "Ready" : "Not ready"}
            </span>
            <span className="text-muted-foreground">
              {status?.status_message ??
                (error ? "Status unavailable" : "Loading status…")}
            </span>
          </span>
          <button
            type="button"
            onClick={() => void load()}
            disabled={loading}
            className="inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-border text-muted-foreground transition hover:bg-muted disabled:opacity-50"
            aria-label="Refresh OpenClaw handoff status"
          >
            <RefreshCw className={`h-4 w-4 ${loading ? "animate-spin" : ""}`} />
          </button>
        </div>

        {error ? (
          <div className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
            <span>{error}</span>
          </div>
        ) : (
          <details className="rounded-lg border border-border bg-muted/40 p-3">
            <summary className="cursor-pointer select-none text-xs font-medium text-muted-foreground">
              Diagnostics
            </summary>
            <div className="mt-3 grid gap-3 text-sm md:grid-cols-2">
            <div>
              <div className="text-xs font-medium uppercase text-muted-foreground">
                Endpoint
              </div>
              <div className="mt-1 break-all font-mono text-xs text-foreground">
                {status?.endpoint || "—"}
              </div>
            </div>
            <div>
              <div className="text-xs font-medium uppercase text-muted-foreground">
                Bearer token
              </div>
              <div className="mt-1 text-foreground">
                {status?.bearer_token_configured ? "Configured" : "Missing"}
              </div>
            </div>
            <div>
              <div className="text-xs font-medium uppercase text-muted-foreground">
                Audio path
              </div>
              <div className="mt-1 text-foreground">
                {status?.include_audio_path ? "Included" : "Not included"}
              </div>
            </div>
            <div>
              <div className="text-xs font-medium uppercase text-muted-foreground">
                Source
              </div>
              <div className="mt-1 font-mono text-xs text-foreground">
                {status?.source || "—"}
              </div>
            </div>
            {status?.last_submission && (
              <div className="rounded-lg bg-muted p-3 md:col-span-2">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="text-xs font-medium uppercase text-muted-foreground">
                    Last handoff
                  </span>
                  <span className="rounded-full bg-background px-2 py-0.5 text-xs font-medium text-foreground">
                    {status.last_submission.state}
                  </span>
                  {status.last_submission.status_code && (
                    <span className="text-xs text-muted-foreground">
                      HTTP {status.last_submission.status_code}
                    </span>
                  )}
                </div>
                <div className="mt-2 text-sm text-foreground">
                  {status.last_submission.message}
                </div>
                <div className="mt-1 text-xs text-muted-foreground">
                  {status.last_submission.updated_at}
                </div>
              </div>
            )}
            </div>
          </details>
        )}
      </div>
    </AddonPanel>
  );
}

// Add-ons: how to react when a Teams meeting is detected. Live detection status
// is in Diagnostics.
function TeamsAutoStartPanel() {
  const [mode, setMode] = useState<TeamsDetectionMode>("off");
  useEffect(() => {
    setMode(getTeamsDetectionMode());
  }, []);
  const onChange = (next: TeamsDetectionMode) => {
    setMode(next);
    setTeamsDetectionMode(next);
  };
  const modeBadge =
    mode === "off"
      ? { label: "Off", classes: "border-border bg-muted text-foreground" }
      : mode === "prompt"
        ? {
            label: "Prompt",
            // Solid pill + white text: readable in both modes and immune to the
            // globals.css `.dark .bg-primary/15` override.
            classes: "border-transparent bg-primary text-white",
          }
        : {
            label: "Auto-record",
            classes: "border-transparent bg-emerald-600 text-white",
          };
  return (
    <AddonPanel
      icon={TeamsIcon}
      title="Teams meeting detection"
      state={mode === "off" ? "planned" : "ready"}
      badgeLabel={modeBadge.label}
      badgeClasses={modeBadge.classes}
      detail="Windows detector for Teams desktop and browser meetings, in several UI languages."
    >
      <div className="space-y-2">
        <label className="block text-sm font-medium text-foreground">
          When a meeting is detected
        </label>
        <select
          className="w-full rounded-lg border border-border bg-muted px-3 py-2 text-sm"
          value={mode}
          onChange={(e) => onChange(e.target.value as TeamsDetectionMode)}
        >
          <option value="off">Do nothing</option>
          <option value="prompt">Prompt me to record</option>
          <option value="auto">Auto-start recording</option>
        </select>
        <p className="text-xs text-muted-foreground">
          {mode === "off"
            ? "Detection runs but takes no action."
            : mode === "prompt"
              ? "Shows a prompt once per detected meeting; you choose whether to record."
              : "Starts a recording once per detected meeting; re-arms when it ends. You can still stop manually."}{" "}
          Live detection status is under Diagnostics.
        </p>
      </div>
    </AddonPanel>
  );
}

// Diagnostics: live Teams meeting-detection signals/status.
function TeamsDetectionPanel() {
  const [teamsStatus, setTeamsStatus] = useState<TeamsDetectionStatus | null>(null);
  const [isCheckingTeams, setIsCheckingTeams] = useState(false);
  const [teamsError, setTeamsError] = useState<string | null>(null);

  const teamsState: AddonState = useMemo(() => {
    if (!teamsStatus) return "connecting";
    if (!teamsStatus.supported) return "planned";
    return teamsStatus.recordingSafety.automaticRecordingAllowed ? "ready" : "advanced";
  }, [teamsStatus]);

  // Detection-status wording (kept distinct from the Add-ons mode dropdown so
  // the two "Teams meeting detection" panels don't look like they should match).
  const teamsBadgeLabel = !teamsStatus
    ? "Checking…"
    : !teamsStatus.supported
      ? "Not supported"
      : teamsStatus.recordingSafety.automaticRecordingAllowed
        ? "Monitoring"
        : "Manual only";

  const checkTeamsDetection = async () => {
    setIsCheckingTeams(true);
    setTeamsError(null);
    try {
      setTeamsStatus(await teamsDetectionService.getStatus());
    } catch (error) {
      setTeamsError(error instanceof Error ? error.message : String(error));
    } finally {
      setIsCheckingTeams(false);
    }
  };

  useEffect(() => {
    void checkTeamsDetection();
  }, []);

  useEffect(() => {
    let cancelled = false;
    const id = window.setInterval(async () => {
      try {
        const status = await teamsDetectionService.getStatus();
        if (!cancelled) {
          setTeamsStatus(status);
          setTeamsError(null);
        }
      } catch {
        // Keep the last good status; the manual button reports failures.
      }
    }, 4000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, []);

  return (
    <AddonPanel
      icon={TeamsIcon}
      title="Teams meeting detection"
      state={teamsState}
      badgeLabel={teamsBadgeLabel}
      detail="Live signals used to detect an active Teams meeting."
    >
      <div className="space-y-3">
        <DetectionSummary status={teamsStatus} />
        {teamsStatus?.reason && (
          <p className="rounded-lg border border-border bg-muted p-3 text-sm text-muted-foreground">
            {teamsStatus.reason}
          </p>
        )}
        {teamsError && (
          <div className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
            <span>{teamsError}</span>
          </div>
        )}
        <div className="flex items-center gap-3">
          <Button
            type="button"
            variant="outline"
            onClick={checkTeamsDetection}
            disabled={isCheckingTeams}
          >
            <RefreshCw className={`mr-2 h-4 w-4 ${isCheckingTeams ? "animate-spin" : ""}`} />
            Refresh now
          </Button>
          <span className="text-xs text-muted-foreground">
            Updates automatically every few seconds.
          </span>
        </div>
      </div>
    </AddonPanel>
  );
}

interface CodexStatus {
  found: boolean;
  authStatus?: string | null;
  accountEmail?: string | null;
  message: string;
}

// Diagnostics: bundled Codex app-server runtime status.
function CodexPanel() {
  const [status, setStatus] = useState<CodexStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setStatus(await invoke<CodexStatus>("codex_check_installation"));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const signedIn = status?.authStatus === "authenticated";
  const ready = !!status?.found;
  const panelState: AddonState = signedIn ? "ready" : ready ? "provider" : "signin";

  return (
    <AddonPanel
      icon={CodexIcon}
      title="Codex app-server"
      state={panelState}
      detail="Bundled local Codex app-server runtime. Configured under Summary → Codex app-server."
    >
      <div className="flex items-center justify-between gap-3">
        <span className="flex flex-wrap items-center gap-2 text-sm">
          <span
            className={`rounded-full px-2.5 py-1 text-xs font-medium ${
              signedIn
                ? "bg-green-100 text-green-800 dark:bg-green-950/40 dark:text-green-200"
                : ready
                  ? "bg-amber-100 text-amber-800 dark:bg-amber-950/40 dark:text-amber-200"
                  : "bg-muted text-muted-foreground"
            }`}
          >
            {signedIn ? "Signed in" : ready ? "Runtime ready" : "Not available"}
          </span>
          <span className="text-muted-foreground">
            {status?.accountEmail ??
              status?.message ??
              (error ? "Status unavailable" : "Loading status…")}
          </span>
        </span>
        <button
          type="button"
          onClick={() => void load()}
          disabled={loading}
          className="inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-border text-muted-foreground transition hover:bg-muted disabled:opacity-50"
          aria-label="Refresh Codex status"
        >
          <RefreshCw className={`h-4 w-4 ${loading ? "animate-spin" : ""}`} />
        </button>
      </div>
      {error && (
        <div className="mt-3 flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
          <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
          <span>{error}</span>
        </div>
      )}
    </AddonPanel>
  );
}

function formatEventTime(start: string | null, end: string | null): string {
  if (!start) return "Time unknown";
  const s = new Date(start);
  const sStr = s.toLocaleString([], {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
  if (!end) return sStr;
  const eStr = new Date(end).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });
  return `${sStr} – ${eStr}`;
}

function attendeeLabel(a: { name: string | null; email: string | null }): string {
  return a.name?.trim() || a.email?.trim() || "Unknown";
}

function attendeeKey(a: { name: string | null; email: string | null }, index: number): string {
  return `${a.email ?? ""}|${a.name ?? ""}|${index}`;
}

const CALENDAR_REFRESH_MS = 5 * 60 * 1000;

// Read-only calendar view: current/next meeting and the next 24h, with the
// invited attendees. Sign-in-gated; reloads on auth changes.
function CalendarPanel() {
  const ms = useMicrosoftExport();
  const isConnected = ms.connection.state === "connected";
  const [usedForNext, setUsedForNext] = useState(false);
  const [attendeeIncluded, setAttendeeIncluded] = useState<Record<string, boolean>>({});

  useEffect(() => {
    if (!isConnected) return;
    let cancelled = false;
    const refresh = () => {
      if (!cancelled) void ms.loadCalendar();
    };
    refresh();
    const interval = window.setInterval(refresh, CALENDAR_REFRESH_MS);
    const onVisibility = () => {
      if (document.visibilityState === "visible") refresh();
    };
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
      window.removeEventListener("focus", refresh);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [isConnected, ms.loadCalendar]);

  const panelState: AddonState = isConnected ? "connected" : "signin";
  const detail = isConnected
    ? "Your current/next meeting and upcoming events, with invited attendees."
    : "Sign in with Microsoft above to see your calendar.";
  const current = ms.currentMeeting;

  // The "Set for your next recording" confirmation belongs to the event shown;
  // reset it whenever the current/next meeting changes (reload or rollover) so
  // it can't claim a different visible event was selected.
  useEffect(() => {
    setUsedForNext(false);
  }, [current?.id]);

  useEffect(() => {
    const next: Record<string, boolean> = {};
    current?.attendees.forEach((attendee, index) => {
      next[attendeeKey(attendee, index)] = true;
    });
    setAttendeeIncluded(next);
  }, [current?.id, current?.attendees]);

  return (
    <AddonPanel
      icon={OutlookCalendarIcon}
      title="Calendar"
      state={panelState}
      detail={detail}
      showBadge={!isConnected}
    >
      {isConnected && (
        <div className="space-y-3">
          <div className="flex items-center justify-between">
            <span className="text-xs font-medium text-muted-foreground">
              Current / next meeting
            </span>
            <button
              type="button"
              className="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground disabled:opacity-50"
              onClick={() => void ms.loadCalendar()}
              disabled={ms.loadingCalendar}
            >
              <RefreshCw className={`h-3 w-3 ${ms.loadingCalendar ? "animate-spin" : ""}`} />
              Reload
            </button>
          </div>

          {current ? (
            <div className="rounded-lg border border-border bg-muted/50 p-3">
              <div className="flex items-center gap-2">
                <span className="truncate text-sm font-medium text-foreground">
                  {current.subject || "(no title)"}
                </span>
                {current.isOnlineMeeting && (
                  <span className="shrink-0 rounded-full border border-transparent bg-primary px-2 py-0.5 text-[10px] font-medium text-white">
                    Online
                  </span>
                )}
              </div>
              <p className="mt-0.5 text-xs text-muted-foreground">
                {formatEventTime(current.start, current.end)}
              </p>
              {current.attendees.length > 0 && (
                <div className="mt-2 space-y-1">
                  <span className="text-xs font-medium text-muted-foreground">
                    Invited attendees
                  </span>
                  <div className="max-h-36 space-y-1 overflow-auto rounded-md border border-border bg-background/60 p-2">
                    {current.attendees.map((attendee, index) => {
                      const key = attendeeKey(attendee, index);
                      const included = attendeeIncluded[key] !== false;
                      return (
                        <label
                          key={key}
                          className={`flex items-center gap-2 text-xs ${
                            included
                              ? "text-foreground"
                              : "text-muted-foreground line-through"
                          }`}
                        >
                          <input
                            type="checkbox"
                            className="h-3.5 w-3.5 accent-primary"
                            checked={included}
                            onChange={(e) =>
                              setAttendeeIncluded((prev) => ({
                                ...prev,
                                [key]: e.target.checked,
                              }))
                            }
                          />
                          <span className="truncate">{attendeeLabel(attendee)}</span>
                        </label>
                      );
                    })}
                  </div>
                </div>
              )}
              <button
                type="button"
                className="mt-2 rounded-md border border-primary/30 bg-primary/10 px-2.5 py-1 text-xs font-medium text-primary hover:bg-primary/20"
                onClick={() => {
                  setPendingCalendar({
                    eventId: current.id,
                    subject: current.subject,
                    attendees: current.attendees.map((attendee, index) => ({
                      ...attendee,
                      included: attendeeIncluded[attendeeKey(attendee, index)] !== false,
                    })),
                  });
                  setUsedForNext(true);
                }}
                title="Title your next recording from this event and add its invited attendees to the summary"
              >
                {usedForNext
                  ? "✓ Set for your next recording"
                  : "Use for next recording"}
              </button>
            </div>
          ) : (
            <p className="rounded-lg border border-border bg-muted/50 p-3 text-xs text-muted-foreground">
              {ms.loadingCalendar
                ? "Loading…"
                : "No meetings scheduled in the next 12 hours."}
            </p>
          )}

          {ms.calendarEvents.length > 0 && (
            <div>
              <span className="mb-1 block text-xs font-medium text-muted-foreground">
                Upcoming (next 24h)
              </span>
              <ul className="space-y-1">
                {ms.calendarEvents.slice(0, 8).map((ev) => (
                  <li
                    key={ev.id}
                    className="flex items-baseline justify-between gap-2 text-xs"
                  >
                    <span className="truncate text-foreground">
                      {ev.subject || "(no title)"}
                    </span>
                    <span className="shrink-0 text-muted-foreground">
                      {formatEventTime(ev.start, ev.end)}
                    </span>
                  </li>
                ))}
              </ul>
            </div>
          )}

          {ms.error && <p className="text-xs text-destructive">{ms.error}</p>}
        </div>
      )}
    </AddonPanel>
  );
}

interface AccelerationDiagnosticStatus {
  compiledBackend: string;
  runtimeDetectedGpu: string;
  useGpu: boolean;
  flashAttn: boolean;
  label: string;
  gpuAvailableButUnused: boolean;
  cpuCores: number;
  memoryGb: number;
  performanceTier: string;
}

interface SystemResourcesDiagnostic {
  memory_used_percent: number;
  cpu_usage_percent: number;
  cpu_temperature_celsius?: number | null;
  available_memory_mb: number;
  total_memory_mb: number;
  cpu_cores: number;
}

interface ModelEngineDiagnostic {
  provider: string;
  loaded?: boolean | null;
  currentModel?: string | null;
  selectedModelPresent?: boolean | null;
  downloadedModels: number;
  totalModels: number;
  modelsDirectory?: string | null;
  error?: string | null;
}

interface TranscriptionDiagnostic {
  selectedProvider: string;
  selectedModel: string;
  apiKeyConfigured: boolean;
  parakeetDirectmlEnabled?: boolean | null;
  engines: ModelEngineDiagnostic[];
}

interface AudioDiagnostic {
  inputDevices: number;
  outputDevices: number;
  totalDevices: number;
  deviceNames: string[];
  error?: string | null;
}

interface RecordingDiagnostic {
  is_recording?: boolean;
  is_paused?: boolean;
  is_active?: boolean;
  recording_duration?: number | null;
  active_duration?: number | null;
  total_pause_duration?: number | null;
  current_pause_duration?: number | null;
}

interface DiarizationRuntimeDiagnostic {
  inProgress: boolean;
  directmlCompiled: boolean;
  directmlFast: boolean;
  directmlSlow: boolean;
  directmlUnavailable: boolean;
  preferredProvider: string;
  runtimeDlls: Array<{
    name: string;
    present: boolean;
    bytes?: number | null;
  }>;
}

interface DiagnosticsSnapshot {
  app: {
    version: string;
    os: string;
    arch: string;
    buildProfile: string;
    appDataDir?: string | null;
    logsDir?: string | null;
  };
  systemResources?: SystemResourcesDiagnostic | null;
  acceleration?: AccelerationDiagnosticStatus | null;
  transcription: TranscriptionDiagnostic;
  audio: AudioDiagnostic;
  recording: RecordingDiagnostic;
  diarization: DiarizationRuntimeDiagnostic;
  integrations: {
    teams?: TeamsDetectionStatus | null;
    openclaw?: OpenClawConfigStatus | null;
    codex?: CodexStatus | null;
  };
  errors: string[];
}

const TRANSCRIPT_PROVIDER_LABELS: Record<string, string> = {
  localWhisper: "Whisper",
  parakeet: "Parakeet",
  nemotron: "Nemotron",
  api: "API",
};

function providerLabel(provider: string): string {
  return TRANSCRIPT_PROVIDER_LABELS[provider] ?? provider;
}

function formatPercent(value?: number | null): string {
  if (typeof value !== "number" || Number.isNaN(value)) return "—";
  return `${value.toFixed(0)}%`;
}

function formatSeconds(value?: number | null): string {
  if (typeof value !== "number" || Number.isNaN(value)) return "—";
  if (value < 60) return `${value.toFixed(0)}s`;
  const minutes = Math.floor(value / 60);
  const seconds = Math.floor(value % 60);
  return `${minutes}m ${seconds}s`;
}

function selectedEngine(snapshot: DiagnosticsSnapshot | null): ModelEngineDiagnostic | null {
  if (!snapshot) return null;
  return (
    snapshot.transcription.engines.find(
      (engine) => engine.provider === snapshot.transcription.selectedProvider,
    ) ?? null
  );
}

function modelState(snapshot: DiagnosticsSnapshot | null): AddonState {
  const engine = selectedEngine(snapshot);
  if (!snapshot || !engine) return "connecting";
  if (engine.error) return "advanced";
  if (engine.selectedModelPresent === false) return "provider";
  return "ready";
}

function diagnosticsState(snapshot: DiagnosticsSnapshot | null): AddonState {
  if (!snapshot) return "connecting";
  if (snapshot.errors.length > 0) return "advanced";
  if (snapshot.audio.inputDevices === 0) return "provider";
  if (snapshot.acceleration?.gpuAvailableButUnused) return "advanced";
  if (modelState(snapshot) !== "ready") return modelState(snapshot);
  return "ready";
}

function diagnosticToneClasses(tone: "good" | "warn" | "neutral" | "bad") {
  switch (tone) {
    case "good":
      return "border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-200";
    case "warn":
      return "border-amber-500/30 bg-amber-500/10 text-amber-800 dark:text-amber-200";
    case "bad":
      return "border-destructive/30 bg-destructive/10 text-destructive";
    case "neutral":
    default:
      return "border-border bg-muted/45 text-foreground";
  }
}

function DiagnosticTile({
  icon: Icon,
  label,
  value,
  detail,
  tone = "neutral",
}: {
  icon: ElementType;
  label: string;
  value: string;
  detail?: string;
  tone?: "good" | "warn" | "neutral" | "bad";
}) {
  return (
    <div className={`rounded-lg border p-3 ${diagnosticToneClasses(tone)}`}>
      <div className="flex items-center gap-2 text-xs font-medium uppercase tracking-wide opacity-80">
        <Icon className="h-3.5 w-3.5" />
        {label}
      </div>
      <div className="mt-2 text-sm font-semibold">{value}</div>
      {detail && <div className="mt-1 text-xs opacity-80">{detail}</div>}
    </div>
  );
}

function redactHomePaths(value: string): string {
  return value
    .replace(
      /(?:\\\\\?\\)?[A-Z]:\\Users\\[^\\/:\r\n]+\\AppData\\Roaming/gi,
      "%APPDATA%",
    )
    .replace(
      /(?:\\\\\?\\)?[A-Z]:\\Users\\[^\\/:\r\n]+\\AppData\\Local/gi,
      "%LOCALAPPDATA%",
    )
    .replace(
      /(?:\\\\\?\\)?[A-Z]:\\Users\\[^\\/:\r\n]+/gi,
      "%USERPROFILE%",
    )
    .replace(/\/Users\/[^/\s]+/g, "~")
    .replace(/\/home\/[^/\s]+/g, "~");
}

function redactEmails(value: string): string {
  return value.replace(
    /\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b/gi,
    "<redacted-email>",
  );
}

function redactEndpoint(value: string): string {
  const trimmed = value.trim();
  if (!trimmed) return value;

  try {
    const url = new URL(trimmed);
    if (url.protocol === "http:" || url.protocol === "https:") {
      return `${url.protocol}//<redacted-host>`;
    }
  } catch {
    // Fall through to a generic placeholder for non-URL endpoint strings.
  }

  return "<redacted-endpoint>";
}

function redactDiagnosticsValue(
  value: unknown,
  key = "",
): unknown {
  if (Array.isArray(value)) {
    return value.map((entry) => redactDiagnosticsValue(entry, key));
  }

  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>).map(
        ([entryKey, entryValue]) => [
          entryKey,
          redactDiagnosticsValue(entryValue, entryKey),
        ],
      ),
    );
  }

  if (typeof value !== "string") return value;

  const normalizedKey = key.toLowerCase();
  if (normalizedKey.includes("email") || normalizedKey === "accountemail") {
    return value.trim() ? "<redacted-email>" : value;
  }

  if (normalizedKey.includes("endpoint") || normalizedKey.endsWith("url")) {
    return redactEndpoint(value);
  }

  return redactEmails(redactHomePaths(value));
}

function sanitizeDiagnosticsForCopy(snapshot: DiagnosticsSnapshot) {
  const copyPayload = {
    generatedAt: new Date().toISOString(),
    app: snapshot.app,
    systemResources: snapshot.systemResources,
    acceleration: snapshot.acceleration,
    transcription: snapshot.transcription,
    audio: {
      ...snapshot.audio,
      deviceNames: snapshot.audio.deviceNames.slice(0, 20),
    },
    recording: snapshot.recording,
    diarization: snapshot.diarization,
    integrations: {
      teams: snapshot.integrations.teams,
      openclaw: snapshot.integrations.openclaw,
      codex: snapshot.integrations.codex,
    },
    errors: snapshot.errors,
  };

  return redactDiagnosticsValue(copyPayload);
}

function DiagnosticsSystemPanel({
  snapshot,
  loading,
  error,
  onRefresh,
}: {
  snapshot: DiagnosticsSnapshot | null;
  loading: boolean;
  error: string | null;
  onRefresh: () => void;
}) {
  const engine = selectedEngine(snapshot);
  const modelStatus = !engine
    ? "Checking"
    : engine.error
      ? "Error"
      : engine.selectedModelPresent === false
        ? "Missing"
        : "Ready";
  const modelTone = modelStatus === "Ready" ? "good" : modelStatus === "Missing" ? "bad" : "warn";
  const accelerationTone = snapshot?.acceleration?.useGpu
    ? "good"
    : snapshot?.acceleration?.gpuAvailableButUnused
      ? "warn"
      : "neutral";
  const audioTone = snapshot?.audio.error
    ? "bad"
    : snapshot && snapshot.audio.inputDevices > 0
      ? "good"
      : "warn";
  const diarizationTone = snapshot?.diarization.directmlFast
    ? "good"
    : snapshot?.diarization.directmlSlow || snapshot?.diarization.directmlUnavailable
      ? "warn"
      : "neutral";

  const copyDiagnostics = async () => {
    if (!snapshot) return;
    try {
      await navigator.clipboard.writeText(
        JSON.stringify(sanitizeDiagnosticsForCopy(snapshot), null, 2),
      );
      toast.success("Diagnostics copied");
    } catch (copyError) {
      toast.error("Failed to copy diagnostics", {
        description: copyError instanceof Error ? copyError.message : String(copyError),
      });
    }
  };

  return (
    <AddonPanel
      icon={ShieldCheck}
      title="System health"
      state={diagnosticsState(snapshot)}
      badgeLabel={snapshot ? stateBadge(diagnosticsState(snapshot)) : "Checking…"}
      detail="Build, engine, audio, and diarization status for support and troubleshooting."
    >
      <div className="space-y-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="text-sm text-muted-foreground">
            {snapshot
              ? `ClawScribe ${snapshot.app.version} · ${snapshot.app.os}/${snapshot.app.arch} · ${snapshot.app.buildProfile}`
              : "Loading local diagnostics…"}
          </div>
          <div className="flex items-center gap-2">
            <Button type="button" variant="outline" onClick={onRefresh} disabled={loading}>
              <RefreshCw className={`mr-2 h-4 w-4 ${loading ? "animate-spin" : ""}`} />
              Refresh
            </Button>
            <Button
              type="button"
              variant="outline"
              onClick={copyDiagnostics}
              disabled={!snapshot}
            >
              <Copy className="mr-2 h-4 w-4" />
              Copy diagnostics
            </Button>
          </div>
        </div>

        {error && (
          <div className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
            <span>{error}</span>
          </div>
        )}

        <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
          <DiagnosticTile
            icon={snapshot?.acceleration?.useGpu ? Zap : Cpu}
            label="Transcription backend"
            value={snapshot?.acceleration?.label ?? "Checking"}
            detail={
              snapshot
                ? `${snapshot.acceleration?.compiledBackend ?? "Unknown"} · GPU: ${
                    snapshot.acceleration?.runtimeDetectedGpu ?? "Unknown"
                  }`
                : undefined
            }
            tone={accelerationTone}
          />
          <DiagnosticTile
            icon={HardDrive}
            label="Selected model"
            value={
              snapshot
                ? `${providerLabel(snapshot.transcription.selectedProvider)} · ${modelStatus}`
                : "Checking"
            }
            detail={snapshot?.transcription.selectedModel}
            tone={modelTone}
          />
          <DiagnosticTile
            icon={Mic}
            label="Audio devices"
            value={
              snapshot
                ? `${snapshot.audio.inputDevices} input · ${snapshot.audio.outputDevices} output`
                : "Checking"
            }
            detail={snapshot?.audio.error ?? `${snapshot?.audio.totalDevices ?? 0} total devices`}
            tone={audioTone}
          />
          <DiagnosticTile
            icon={Gauge}
            label="Diarization"
            value={
              snapshot
                ? snapshot.diarization.inProgress
                  ? "Running"
                  : snapshot.diarization.preferredProvider.toUpperCase()
                : "Checking"
            }
            detail={
              snapshot
                ? snapshot.diarization.directmlFast
                  ? "DirectML accepted this session"
                  : snapshot.diarization.directmlSlow
                    ? "DirectML probed slow; CPU fallback"
                    : snapshot.diarization.directmlUnavailable
                      ? "DirectML unavailable; CPU fallback"
                      : `${snapshot.diarization.runtimeDlls.filter((dll) => dll.present).length}/${
                          snapshot.diarization.runtimeDlls.length
                        } runtime files present`
                : undefined
            }
            tone={diarizationTone}
          />
        </div>

        {snapshot?.acceleration?.gpuAvailableButUnused && (
          <div className="flex items-start gap-2 rounded-lg border border-amber-500/30 bg-amber-500/10 p-3 text-sm text-amber-800 dark:text-amber-200">
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
            <span>
              A {snapshot.acceleration.runtimeDetectedGpu} GPU is visible, but this build is using
              CPU transcription.
            </span>
          </div>
        )}

        {snapshot && (
          <details className="rounded-lg border border-border bg-muted/35 p-3">
            <summary className="cursor-pointer select-none text-xs font-medium text-muted-foreground">
              Details
            </summary>
            <div className="mt-3 grid gap-3 text-sm lg:grid-cols-2">
              <div className="space-y-2">
                <div className="text-xs font-medium uppercase text-muted-foreground">
                  System load
                </div>
                <div className="rounded-md border border-border bg-background/70 p-3 text-xs text-muted-foreground">
                  CPU {formatPercent(snapshot.systemResources?.cpu_usage_percent)} · Memory{" "}
                  {formatPercent(snapshot.systemResources?.memory_used_percent)} ·{" "}
                  {snapshot.systemResources?.cpu_cores ?? snapshot.acceleration?.cpuCores ?? "—"}{" "}
                  cores
                </div>
              </div>
              <div className="space-y-2">
                <div className="text-xs font-medium uppercase text-muted-foreground">
                  Recording
                </div>
                <div className="rounded-md border border-border bg-background/70 p-3 text-xs text-muted-foreground">
                  {snapshot.recording.is_recording ? "Recording" : "Idle"} ·{" "}
                  {snapshot.recording.is_paused ? "Paused" : "Active-ready"} · active{" "}
                  {formatSeconds(snapshot.recording.active_duration)}
                </div>
              </div>
              <div className="space-y-2">
                <div className="text-xs font-medium uppercase text-muted-foreground">
                  Models
                </div>
                <div className="space-y-1 rounded-md border border-border bg-background/70 p-3 text-xs text-muted-foreground">
                  {snapshot.transcription.engines.map((engine) => (
                    <div key={engine.provider} className="flex justify-between gap-3">
                      <span>{providerLabel(engine.provider)}</span>
                      <span>
                        {engine.downloadedModels}/{engine.totalModels} downloaded
                        {engine.loaded ? " · loaded" : ""}
                      </span>
                    </div>
                  ))}
                </div>
              </div>
              <div className="space-y-2">
                <div className="text-xs font-medium uppercase text-muted-foreground">
                  Runtime paths
                </div>
                <div className="space-y-1 rounded-md border border-border bg-background/70 p-3 font-mono text-[11px] text-muted-foreground">
                  <div className="break-all">Data: {snapshot.app.appDataDir ?? "—"}</div>
                  <div className="break-all">Logs: {snapshot.app.logsDir ?? "—"}</div>
                </div>
              </div>
            </div>
            {snapshot.errors.length > 0 && (
              <div className="mt-3 rounded-md border border-amber-500/30 bg-amber-500/10 p-3 text-xs text-amber-800 dark:text-amber-200">
                {snapshot.errors.map((item) => (
                  <div key={item}>{item}</div>
                ))}
              </div>
            )}
          </details>
        )}
      </div>
    </AddonPanel>
  );
}

function GroupHeader({ icon, title, desc }: { icon: ReactNode; title: string; desc: string }) {
  return (
    <div className="flex items-start gap-3 border-b border-border pb-3">
      <span className="mt-0.5 flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-border bg-background text-primary shadow-sm">
        {icon}
      </span>
      <div>
        <h2 className="text-base font-semibold tracking-tight text-foreground">{title}</h2>
        <p className="mt-0.5 text-sm text-muted-foreground">{desc}</p>
      </div>
    </div>
  );
}

export function IntegrationsSettings() {
  return (
    <div className="space-y-8">
      {/* Microsoft 365 — sign-in is the gateway, exports depend on it. */}
      <section className="space-y-4">
        <GroupHeader
          icon={<Microsoft365Icon />}
          title="Microsoft 365"
          desc="Sign in with your work account to export meetings to OneNote, OneDrive, Planner, and To Do, and pull in calendar events."
        />
        <div className="space-y-4">
          <MicrosoftSignInPanel />
          <OneNotePanel />
          <OneDrivePanel />
          <PlannerPanel />
          <ToDoPanel />
          <CalendarPanel />
        </div>
      </section>

      <section className="space-y-4">
        <GroupHeader
          icon={<ConfluenceIcon />}
          title="Confluence"
          desc="Copy a Confluence-ready draft or publish directly with your configured REST endpoint."
        />
        <ConfluencePanel />
      </section>

      {/* Meeting detection — a recording trigger, not an export destination. */}
      <section className="space-y-4">
        <GroupHeader
          icon={<TeamsIcon />}
          title="Meeting detection"
          desc="Auto-start recording when a Teams meeting is detected. Live status is under Diagnostics."
        />
        <TeamsAutoStartPanel />
      </section>
    </div>
  );
}

export function DiagnosticsSettings() {
  const [snapshot, setSnapshot] = useState<DiagnosticsSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const loadSnapshot = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setSnapshot(await invoke<DiagnosticsSnapshot>("get_diagnostics_snapshot"));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadSnapshot();
  }, [loadSnapshot]);

  return (
    <div className="space-y-5">
      <div className="rounded-lg border border-border bg-card p-5 shadow-sm">
        <div className="flex items-start gap-3">
          <Activity className="mt-0.5 h-5 w-5 text-primary" />
          <div>
            <h2 className="text-lg font-semibold text-foreground">Diagnostics</h2>
            <p className="mt-1 text-sm text-muted-foreground">
              Build, model, audio, diarization, and integration health for
              troubleshooting.
            </p>
          </div>
        </div>
      </div>

      <DiagnosticsSystemPanel
        snapshot={snapshot}
        loading={loading}
        error={error}
        onRefresh={loadSnapshot}
      />
      <TeamsDetectionPanel />
      <OpenClawPanel />
      <CodexPanel />
    </div>
  );
}
