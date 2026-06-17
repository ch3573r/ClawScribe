"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import type { ElementType, ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  AlertTriangle,
  CheckCircle2,
  Cloud,
  FileCheck2,
  ListTodo,
  Loader2,
  LogIn,
  LogOut,
  NotebookTabs,
  RefreshCw,
  ShieldCheck,
  User,
  Video,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  teamsDetectionService,
  TeamsDetectionStatus,
} from "@/services/teamsDetectionService";
import { useMicrosoftExport } from "@/hooks/useMicrosoftExport";
import {
  getExportDestinations,
  setExportDestinations,
} from "@/lib/exportDestinations";
import { getAutoRecordEnabled, setAutoRecordEnabled } from "@/lib/autoRecord";

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
    case "ready":
    case "connected":
      return "border-emerald-200 bg-emerald-50 text-emerald-800 dark:border-emerald-900 dark:bg-emerald-950/40 dark:text-emerald-200";
    case "connecting":
    case "signin":
      return "border-blue-200 bg-blue-50 text-blue-800 dark:border-blue-900 dark:bg-blue-950/40 dark:text-blue-200";
    case "prompt":
      return "border-blue-200 bg-blue-50 text-blue-800 dark:border-blue-900 dark:bg-blue-950/40 dark:text-blue-200";
    case "provider":
      return "border-primary/20 bg-primary/10 text-primary";
    case "advanced":
      return "border-amber-200 bg-amber-50 text-amber-800 dark:border-amber-900 dark:bg-amber-950/40 dark:text-amber-200";
    case "planned":
    default:
      return "border-muted bg-muted text-muted-foreground";
  }
}

interface AddonPanelProps {
  icon: ElementType;
  title: string;
  state: AddonState;
  detail: string;
  children?: ReactNode;
}

function AddonPanel({
  icon: Icon,
  title,
  state,
  detail,
  children,
}: AddonPanelProps) {
  return (
    <section className="rounded-lg border border-border bg-card p-5 shadow-sm">
      <div className="flex items-start justify-between gap-4">
        <div className="flex min-w-0 items-start gap-3">
          <div className="rounded-lg border border-border bg-muted p-2">
            <Icon className="h-5 w-5 text-primary" />
          </div>
          <div className="min-w-0">
            <h3 className="text-base font-semibold text-foreground">{title}</h3>
            <p className="mt-1 text-sm text-muted-foreground">{detail}</p>
          </div>
        </div>
        <span
          className={`shrink-0 rounded-full border px-2.5 py-1 text-xs font-medium ${stateClasses(state)}`}
        >
          {stateBadge(state)}
        </span>
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
      return `Signed in as ${ms.connection.userDisplayName ?? ms.connection.userEmail ?? "Microsoft user"}. OneNote and Planner exports are available.`;
    }
    if (ms.connection.state === "connecting" || ms.signingIn) {
      return "Waiting for Microsoft sign-in to complete…";
    }
    if (ms.connection.state === "expired") {
      return "Microsoft session expired. Sign in again to re-enable exports.";
    }
    return "Sign in with your Microsoft account to enable OneNote and Planner exports.";
  }, [ms.connection, ms.signingIn]);

  return (
    <AddonPanel
      icon={User}
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
                    . Sign out and sign in again to grant OneNote/Planner
                    access; if the consent screen does not list them, the Entra
                    app registration needs those Graph permissions and admin
                    consent.
                  </span>
                </div>
              )}
          </>
        )}

        {(ms.connection.state === "connecting" || ms.signingIn) && (
          <div className="rounded-lg border border-blue-200 bg-blue-50 p-4 dark:border-blue-900 dark:bg-blue-950/40">
            <div className="flex items-center gap-2 text-sm text-blue-800 dark:text-blue-200">
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

  // Persist the chosen notebook. The section is created per-export (a dated
  // section), so there is no section picker — this also sidesteps the OneNote
  // 5,000-items-per-library enumeration limit, which only affects listing.
  useEffect(() => {
    if (!selectedNotebook) return;
    setExportDestinations({
      notebookId: selectedNotebook,
      notebookName: ms.notebooks.find((n) => n.id === selectedNotebook)
        ?.displayName,
      sectionId: undefined,
      sectionName: undefined,
    });
  }, [selectedNotebook, ms.notebooks]);

  const panelState: AddonState = isConnected ? "connected" : "signin";
  const detail = isConnected
    ? "Pick a notebook. Each export creates a new dated section in it."
    : "Sign in with Microsoft above to enable OneNote export.";

  return (
    <AddonPanel
      icon={NotebookTabs}
      title="OneNote export"
      state={panelState}
      detail={detail}
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
              <option value={NEW_OPTION}>+ New notebook…</option>
            </select>
          </div>

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
                OneNote destination ready. Export from a meeting&apos;s summary
                panel.
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
      icon={ListTodo}
      title="Planner task export"
      state={panelState}
      detail={detail}
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
      icon={Cloud}
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
          <div className="grid gap-3 text-sm md:grid-cols-2">
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
        )}
      </div>
    </AddonPanel>
  );
}

export function IntegrationsSettings() {
  const [teamsStatus, setTeamsStatus] = useState<TeamsDetectionStatus | null>(
    null,
  );
  const [isCheckingTeams, setIsCheckingTeams] = useState(false);
  const [teamsError, setTeamsError] = useState<string | null>(null);
  const [autoRecord, setAutoRecord] = useState(false);

  useEffect(() => {
    setAutoRecord(getAutoRecordEnabled());
  }, []);

  const toggleAutoRecord = () => {
    const next = !autoRecord;
    setAutoRecord(next);
    setAutoRecordEnabled(next);
  };

  const teamsState: AddonState = useMemo(() => {
    if (!teamsStatus) return "prompt";
    if (!teamsStatus.supported) return "planned";
    return teamsStatus.recordingSafety.automaticRecordingAllowed
      ? "ready"
      : "prompt";
  }, [teamsStatus]);

  const checkTeamsDetection = async () => {
    setIsCheckingTeams(true);
    setTeamsError(null);
    try {
      const status = await teamsDetectionService.getStatus();
      setTeamsStatus(status);
    } catch (error) {
      setTeamsError(error instanceof Error ? error.message : String(error));
    } finally {
      setIsCheckingTeams(false);
    }
  };

  useEffect(() => {
    void checkTeamsDetection();
  }, []);

  // Auto-refresh the detection status while the panel is open so it reflects the
  // live state without a manual click. Silent (no spinner / error flash) — the
  // "Refresh now" button still surfaces errors explicitly.
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
    <div className="space-y-5">
      <div className="rounded-lg border border-border bg-card p-5 shadow-sm">
        <div className="flex items-start gap-3">
          <ShieldCheck className="mt-0.5 h-5 w-5 text-primary" />
          <div>
            <h2 className="text-lg font-semibold text-foreground">
              Add-ons and integrations
            </h2>
            <p className="mt-1 text-sm text-muted-foreground">
              Current status for meeting detection, handoff, exports, and
              advanced providers.
            </p>
          </div>
        </div>
      </div>

      <AddonPanel
        icon={Video}
        title="Teams meeting detection"
        state={teamsState}
        detail="Windows detector for Teams desktop and browser meetings. Detects meetings in several UI languages."
      >
        <div className="space-y-3">
          <label className="flex cursor-pointer items-start justify-between gap-4 rounded-lg border border-border bg-muted p-3">
            <span>
              <span className="block text-sm font-medium text-foreground">
                Auto-start recording when a meeting is detected
              </span>
              <span className="mt-0.5 block text-xs text-muted-foreground">
                Starts a recording once per detected meeting; re-arms when the
                meeting ends. You can still stop manually.
              </span>
            </span>
            <button
              type="button"
              role="switch"
              aria-checked={autoRecord}
              onClick={toggleAutoRecord}
              className={`relative mt-0.5 inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors ${
                autoRecord ? "bg-primary" : "bg-border"
              }`}
            >
              <span
                className={`inline-block h-5 w-5 transform rounded-full bg-background shadow transition-transform ${
                  autoRecord ? "translate-x-5" : "translate-x-0.5"
                }`}
              />
            </button>
          </label>

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
              <RefreshCw
                className={`mr-2 h-4 w-4 ${isCheckingTeams ? "animate-spin" : ""}`}
              />
              Refresh now
            </Button>
            <span className="text-xs text-muted-foreground">
              Updates automatically every few seconds.
            </span>
          </div>
        </div>
      </AddonPanel>

      <OpenClawPanel />

      <MicrosoftSignInPanel />

      <OneNotePanel />

      <PlannerPanel />

      <AddonPanel
        icon={FileCheck2}
        title="Advanced: Codex app-server"
        state="advanced"
        detail="Configured from Summary → Advanced: Codex app-server. This is a bundled runtime provider, not a global Codex CLI integration."
      />
    </div>
  );
}
