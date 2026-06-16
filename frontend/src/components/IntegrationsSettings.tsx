"use client";

import { useEffect, useMemo, useState } from "react";
import type { ElementType, ReactNode } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  Cloud,
  Copy,
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
import { teamsDetectionService, TeamsDetectionStatus } from "@/services/teamsDetectionService";
import { useMicrosoftExport } from "@/hooks/useMicrosoftExport";

type AddonState = "ready" | "prompt" | "planned" | "provider" | "advanced" | "connected" | "connecting";

function stateBadge(state: AddonState) {
  switch (state) {
    case "ready":
    case "connected":
      return "Ready";
    case "connecting":
      return "Connecting…";
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

function AddonPanel({ icon: Icon, title, state, detail, children }: AddonPanelProps) {
  return (
    <section className="rounded-lg border border-border bg-card p-5 shadow-sm">
      <div className="flex items-start justify-between gap-4">
        <div className="flex min-w-0 items-start gap-3">
          <div className="rounded-md border border-border bg-background p-2">
            <Icon className="h-5 w-5 text-primary" />
          </div>
          <div className="min-w-0">
            <h3 className="text-base font-semibold text-card-foreground">{title}</h3>
            <p className="mt-1 text-sm text-muted-foreground">{detail}</p>
          </div>
        </div>
        <span className={`shrink-0 rounded-full border px-2.5 py-1 text-xs font-medium ${stateClasses(state)}`}>
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
      <div className="rounded-md border border-border bg-background p-3">
        <p className="text-xs text-muted-foreground">Platform</p>
        <p className="font-medium text-foreground">{status.platform}</p>
      </div>
      <div className="rounded-md border border-border bg-background p-3">
        <p className="text-xs text-muted-foreground">Status</p>
        <p className="font-medium text-foreground">{status.status}</p>
      </div>
      <div className="rounded-md border border-border bg-background p-3">
        <p className="text-xs text-muted-foreground">Confidence</p>
        <p className="font-medium text-foreground">{Math.round(status.confidence * 100)}%</p>
      </div>
      <div className="rounded-md border border-border bg-background p-3">
        <p className="text-xs text-muted-foreground">Action</p>
        <p className="font-medium text-foreground">{status.nextRecommendedAction}</p>
      </div>
    </div>
  );
}

function MicrosoftSignInPanel() {
  const ms = useMicrosoftExport();

  const panelState: AddonState = useMemo(() => {
    if (ms.connection.state === "connected") return "connected";
    if (ms.connection.state === "connecting" || ms.signingIn) return "connecting";
    return "planned";
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
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2 text-sm text-muted-foreground">
              <CheckCircle2 className="h-4 w-4 text-emerald-600" />
              <span>
                {ms.connection.userDisplayName}
                {ms.connection.userEmail && (
                  <span className="ml-1 text-xs opacity-70">({ms.connection.userEmail})</span>
                )}
              </span>
            </div>
            <Button type="button" variant="outline" size="sm" onClick={ms.signOut}>
              <LogOut className="mr-2 h-4 w-4" />
              Sign out
            </Button>
          </div>
        )}

        {(ms.connection.state === "connecting" || ms.signingIn) && ms.userCode && (
          <div className="rounded-md border border-blue-200 bg-blue-50 p-4 dark:border-blue-900 dark:bg-blue-950/40">
            <p className="mb-2 text-sm text-blue-800 dark:text-blue-200">
              Enter this code at <strong>microsoft.com/devicelogin</strong>:
            </p>
            <div className="flex items-center gap-2">
              <code className="rounded bg-white px-3 py-1.5 text-lg font-bold tracking-widest text-blue-900 dark:bg-blue-900 dark:text-blue-100">
                {ms.userCode}
              </code>
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => navigator.clipboard.writeText(ms.userCode!)}
              >
                <Copy className="h-4 w-4" />
              </Button>
            </div>
            <div className="mt-2 flex items-center gap-2 text-xs text-blue-600 dark:text-blue-300">
              <Loader2 className="h-3 w-3 animate-spin" />
              <span>Waiting for sign-in…</span>
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
          <div className="flex items-start gap-2 rounded-md border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
            <span>{ms.error}</span>
          </div>
        )}
      </div>
    </AddonPanel>
  );
}

function OneNotePanel() {
  const ms = useMicrosoftExport();
  const [selectedNotebook, setSelectedNotebook] = useState<string>("");
  const [selectedSection, setSelectedSection] = useState<string>("");

  const isConnected = ms.connection.state === "connected";

  useEffect(() => {
    if (isConnected && ms.notebooks.length === 0 && !ms.loadingNotebooks) {
      void ms.loadNotebooks();
    }
  }, [isConnected]);

  useEffect(() => {
    if (selectedNotebook) {
      void ms.loadSections(selectedNotebook);
      setSelectedSection("");
    }
  }, [selectedNotebook]);

  const panelState: AddonState = isConnected ? "connected" : "planned";
  const detail = isConnected
    ? "Select a notebook and section for meeting note exports."
    : "Sign in with Microsoft above to enable OneNote export.";

  return (
    <AddonPanel icon={NotebookTabs} title="OneNote export" state={panelState} detail={detail}>
      {isConnected && (
        <div className="space-y-3">
          <div className="grid gap-3 sm:grid-cols-2">
            <div>
              <label className="mb-1 block text-xs font-medium text-muted-foreground">
                Notebook
              </label>
              <select
                className="w-full rounded-md border border-border bg-background px-3 py-2 text-sm"
                value={selectedNotebook}
                onChange={(e) => setSelectedNotebook(e.target.value)}
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
              </select>
            </div>
            <div>
              <label className="mb-1 block text-xs font-medium text-muted-foreground">
                Section
              </label>
              <select
                className="w-full rounded-md border border-border bg-background px-3 py-2 text-sm"
                value={selectedSection}
                onChange={(e) => setSelectedSection(e.target.value)}
                disabled={!selectedNotebook || ms.loadingSections}
              >
                <option value="">
                  {ms.loadingSections
                    ? "Loading…"
                    : !selectedNotebook
                      ? "Select a notebook first"
                      : "Select a section"}
                </option>
                {ms.sections.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.displayName}
                  </option>
                ))}
              </select>
            </div>
          </div>
          {selectedSection && (
            <div className="flex items-center gap-2 text-sm text-emerald-700 dark:text-emerald-300">
              <CheckCircle2 className="h-4 w-4" />
              <span>
                OneNote destination ready. Export from a meeting&apos;s summary panel.
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
  const [selectedPlan, setSelectedPlan] = useState<string>("");
  const [selectedBucket, setSelectedBucket] = useState<string>("");

  const isConnected = ms.connection.state === "connected";

  useEffect(() => {
    if (isConnected && ms.plans.length === 0 && !ms.loadingPlans) {
      void ms.loadPlans();
    }
  }, [isConnected]);

  useEffect(() => {
    if (selectedPlan) {
      void ms.loadBuckets(selectedPlan);
      setSelectedBucket("");
    }
  }, [selectedPlan]);

  const panelState: AddonState = isConnected ? "connected" : "planned";
  const detail = isConnected
    ? "Select a plan and bucket for action item exports."
    : "Sign in with Microsoft above to enable Planner export.";

  return (
    <AddonPanel icon={ListTodo} title="Planner task export" state={panelState} detail={detail}>
      {isConnected && (
        <div className="space-y-3">
          <div className="grid gap-3 sm:grid-cols-2">
            <div>
              <label className="mb-1 block text-xs font-medium text-muted-foreground">
                Plan
              </label>
              <select
                className="w-full rounded-md border border-border bg-background px-3 py-2 text-sm"
                value={selectedPlan}
                onChange={(e) => setSelectedPlan(e.target.value)}
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
                Bucket
              </label>
              <select
                className="w-full rounded-md border border-border bg-background px-3 py-2 text-sm"
                value={selectedBucket}
                onChange={(e) => setSelectedBucket(e.target.value)}
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
              </select>
            </div>
          </div>
          {selectedBucket && (
            <div className="flex items-center gap-2 text-sm text-emerald-700 dark:text-emerald-300">
              <CheckCircle2 className="h-4 w-4" />
              <span>
                Planner destination ready. Export from a meeting&apos;s summary panel.
              </span>
            </div>
          )}
        </div>
      )}
    </AddonPanel>
  );
}

export function IntegrationsSettings() {
  const [teamsStatus, setTeamsStatus] = useState<TeamsDetectionStatus | null>(null);
  const [isCheckingTeams, setIsCheckingTeams] = useState(false);
  const [teamsError, setTeamsError] = useState<string | null>(null);

  const teamsState: AddonState = useMemo(() => {
    if (!teamsStatus) return "prompt";
    if (!teamsStatus.supported) return "planned";
    return teamsStatus.recordingSafety.automaticRecordingAllowed ? "ready" : "prompt";
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

  return (
    <div className="space-y-5">
      <div className="rounded-lg border border-border bg-card p-5">
        <div className="flex items-start gap-3">
          <ShieldCheck className="mt-0.5 h-5 w-5 text-primary" />
          <div>
            <h2 className="text-lg font-semibold text-card-foreground">Add-ons and integrations</h2>
            <p className="mt-1 text-sm text-muted-foreground">
              Current status for meeting detection, handoff, exports, and advanced providers.
            </p>
          </div>
        </div>
      </div>

      <AddonPanel
        icon={Video}
        title="Teams meeting detection"
        state={teamsState}
        detail="Windows detector for Teams desktop and browser meetings. Current safety mode recommends recording; it does not auto-start recording yet."
      >
        <div className="space-y-3">
          <DetectionSummary status={teamsStatus} />
          {teamsStatus?.reason && (
            <p className="rounded-md border border-border bg-background p-3 text-sm text-muted-foreground">
              {teamsStatus.reason}
            </p>
          )}
          {teamsError && (
            <div className="flex items-start gap-2 rounded-md border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
              <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
              <span>{teamsError}</span>
            </div>
          )}
          <Button type="button" variant="outline" onClick={checkTeamsDetection} disabled={isCheckingTeams}>
            <RefreshCw className={`mr-2 h-4 w-4 ${isCheckingTeams ? "animate-spin" : ""}`} />
            Check Teams detection
          </Button>
        </div>
      </AddonPanel>

      <AddonPanel
        icon={Cloud}
        title="OpenClaw handoff"
        state="provider"
        detail="Configured from Summary → OpenClaw provider. It can receive meeting.completed payloads and return the same notes contract as the other providers."
      >
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <CheckCircle2 className="h-4 w-4 text-emerald-600" />
          <span>Available as a summary provider and handoff target.</span>
        </div>
      </AddonPanel>

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
