"use client";

import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import type { ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, Copy, RefreshCw } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Accordion, AccordionContent, AccordionItem, AccordionTrigger } from "@/components/ui/accordion";
import {
  Meta, SettingsCard, SettingsColumns, SettingsFooter, SettingsRow, StatusText,
} from "@/components/ui/settings-layout";
import { IconAction, NoteRow } from "@/components/settings/settings-parts";
import { clockTime, describeCallCheck, useCallDetectionCheck } from "@/components/settings/call-detection-check";
import { cn } from "@/lib/utils";
import { getModelDisplayName } from "@/lib/parakeet";
import {
  CALL_PROVIDERS, CALL_SETTINGS_CHANGED, getCallSettings, type CallProvider,
} from "@/lib/autoRecord";
import {
  getICalendarServerSnapshot, getICalendarSnapshot, refreshICalendars, subscribeToICalendars,
} from "@/services/icalService";
import type { TeamsDetectionStatus } from "@/services/teamsDetectionService";

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

interface CodexStatus {
  found: boolean;
  authStatus?: string | null;
  accountEmail?: string | null;
  message: string;
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

export interface DiagnosticsSnapshot {
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

/** Runtime identifiers arrive in lower case; show them the way the vendors write them. */
function runtimeLabel(runtime: string): string {
  return ({ directml: "DirectML", cpu: "CPU", cuda: "CUDA", coreml: "Core ML" } as Record<string, string>)[runtime.toLowerCase()] ?? runtime;
}

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

export function redactDiagnosticsValue(
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

export function sanitizeDiagnosticsForCopy(snapshot: DiagnosticsSnapshot) {
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

type HealthTone = "ok" | "warning" | "error";

interface HealthItem {
  id: string;
  name: string;
  value: ReactNode;
  tone: HealthTone;
  /** Short status word. Fine rows show it quietly, problems colour it. */
  status: string;
  action?: ReactNode;
}

const toneOrder: Record<HealthTone, number> = { error: 0, warning: 1, ok: 2 };

/** Problems first; rows with the same tone keep their natural order. */
export function sortHealthItems<T extends { tone: HealthTone }>(items: T[]): T[] {
  return items
    .map((item, index) => ({ item, index }))
    .sort((a, b) => toneOrder[a.item.tone] - toneOrder[b.item.tone] || a.index - b.index)
    .map(({ item }) => item);
}

function HealthRow({ item }: { item: HealthItem }) {
  const problem = item.tone !== "ok";
  return (
    <div className={cn(
      "flex min-h-16 flex-wrap items-center gap-x-6 gap-y-1 px-5 py-3",
      item.tone === "warning" && "bg-warning text-warning-foreground",
      item.tone === "error" && "bg-error text-error-foreground",
    )}>
      <div className={cn("w-52 shrink-0 text-sm", !problem && "text-foreground")}>{item.name}</div>
      <div className={cn("min-w-0 flex-1 basis-48 break-words text-sm", !problem && "text-muted-foreground")}>{item.value}</div>
      <div className="flex shrink-0 items-center gap-2">
        {problem
          ? <StatusText tone={item.tone}>{item.status}</StatusText>
          : <span className="inline-flex items-center gap-1.5 whitespace-nowrap text-sm text-muted-foreground"><Check aria-hidden="true" className="h-3.5 w-3.5" />{item.status}</span>}
        {item.action}
      </div>
    </div>
  );
}

function useOpenClawStatus() {
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
  useEffect(() => { void load(); }, [load]);
  return { status, error, loading, load };
}

function useCodexStatus() {
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
  useEffect(() => { void load(); }, [load]);
  return { status, error, loading, load };
}

function DetailBlock({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="grid gap-1">
      <div className="text-xs text-muted-foreground">{title}</div>
      <div className="grid gap-0.5 text-[13px] text-foreground">{children}</div>
    </div>
  );
}

function DiagnosticsDetails({ snapshot, openclaw }: { snapshot: DiagnosticsSnapshot | null; openclaw: OpenClawConfigStatus | null }) {
  if (!snapshot && !openclaw) return null;
  return (
    <Accordion type="single" collapsible className="px-5">
      <AccordionItem value="details" className="border-0">
        <AccordionTrigger className="py-3 text-[13px] font-normal text-muted-foreground hover:no-underline">Details</AccordionTrigger>
        <AccordionContent>
          <div className="grid gap-4 pb-2 lg:grid-cols-2">
            {snapshot && (
              <>
                <DetailBlock title="System load">
                  <span>
                    CPU {formatPercent(snapshot.systemResources?.cpu_usage_percent)} · Memory{" "}
                    {formatPercent(snapshot.systemResources?.memory_used_percent)} ·{" "}
                    {snapshot.systemResources?.cpu_cores ?? snapshot.acceleration?.cpuCores ?? "—"} cores
                  </span>
                </DetailBlock>
                <DetailBlock title="Recording">
                  <span>
                    {snapshot.recording.is_recording ? "Recording" : "Idle"} ·{" "}
                    {snapshot.recording.is_paused ? "Paused" : "Active-ready"} · active{" "}
                    {formatSeconds(snapshot.recording.active_duration)}
                  </span>
                </DetailBlock>
                <DetailBlock title="Models">
                  {snapshot.transcription.engines.map((engine) => (
                    <div key={engine.provider} className="flex justify-between gap-3">
                      <span>{providerLabel(engine.provider)}</span>
                      <span className="text-muted-foreground">
                        {engine.downloadedModels}/{engine.totalModels} downloaded{engine.loaded ? " · loaded" : ""}
                      </span>
                    </div>
                  ))}
                </DetailBlock>
                <DetailBlock title="Runtime paths">
                  <span className="break-all font-mono text-xs">Data: {snapshot.app.appDataDir ?? "—"}</span>
                  <span className="break-all font-mono text-xs">Logs: {snapshot.app.logsDir ?? "—"}</span>
                </DetailBlock>
                {snapshot.errors.length > 0 && (
                  <DetailBlock title="Collection errors">
                    {snapshot.errors.map((item) => <span key={item} className="text-warning-foreground">{item}</span>)}
                  </DetailBlock>
                )}
              </>
            )}
            {openclaw && (
              <DetailBlock title="OpenClaw handoff">
                <span className="break-all font-mono text-xs">Endpoint: {openclaw.endpoint || "—"}</span>
                <span>Bearer token: {openclaw.bearer_token_configured ? "Configured" : "Missing"}</span>
                <span>Audio path: {openclaw.include_audio_path ? "Included" : "Not included"}</span>
                <span className="font-mono text-xs">Source: {openclaw.source || "—"}</span>
                {openclaw.last_submission && (
                  <span>
                    Last handoff: {openclaw.last_submission.state}
                    {openclaw.last_submission.status_code ? ` · HTTP ${openclaw.last_submission.status_code}` : ""}
                    {" · "}{openclaw.last_submission.message}
                    {" · "}{openclaw.last_submission.updated_at}
                  </span>
                )}
              </DetailBlock>
            )}
          </div>
        </AccordionContent>
      </AccordionItem>
    </Accordion>
  );
}

function CallDetectionCard() {
  const [providers, setProviders] = useState<CallProvider[]>(["teams"]);
  const { state, check, reset } = useCallDetectionCheck(providers);

  useEffect(() => {
    const read = () => setProviders(getCallSettings().providers);
    read();
    window.addEventListener(CALL_SETTINGS_CHANGED, read);
    return () => window.removeEventListener(CALL_SETTINGS_CHANGED, read);
  }, []);

  // The apps changed (or the page just opened): scan once with the current selection.
  useEffect(() => {
    reset();
    if (providers.length) void check();
  }, [check, reset, providers]);

  const result = describeCallCheck(state);
  return (
    <SettingsCard title="Call detection" description="The same check that starts recordings">
      <SettingsRow title="Result"><span role="status"><StatusText tone={result.tone}>{result.text}</StatusText></span></SettingsRow>
      <SettingsRow title="Apps watched">
        <span className="text-sm text-muted-foreground">
          {providers.length ? providers.map((provider) => CALL_PROVIDERS[provider]).join(", ") : "None selected"}
        </span>
      </SettingsRow>
      <SettingsRow title="Windows checked">
        <Meta>{state.scan?.supported ? state.scan.openWindows.length : "—"}</Meta>
      </SettingsRow>
      <SettingsRow title="Last check">
        <Meta>{state.checkedAt ? clockTime(state.checkedAt) : "Not checked yet"}</Meta>
      </SettingsRow>
      <SettingsFooter
        actions={
          <Button type="button" variant="ghost" size="sm" disabled={state.checking || !providers.length} onClick={() => void check()}>
            {state.checking ? "Checking…" : "Check now"}
          </Button>
        }
      >
        {!providers.length && <span>Select a meeting app on the Recording page to check</span>}
      </SettingsFooter>
    </SettingsCard>
  );
}

export function DiagnosticsSettings() {
  const [snapshot, setSnapshot] = useState<DiagnosticsSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [savingReport, setSavingReport] = useState(false);
  const saveInProgress = useRef(false);
  const openclaw = useOpenClawStatus();
  const codex = useCodexStatus();
  const calendars = useSyncExternalStore(subscribeToICalendars, getICalendarSnapshot, getICalendarServerSnapshot);

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

  const refreshAll = () => {
    void loadSnapshot();
    void openclaw.load();
    void codex.load();
  };

  const copyDiagnostics = async () => {
    if (!snapshot) return;
    try {
      await navigator.clipboard.writeText(JSON.stringify(sanitizeDiagnosticsForCopy(snapshot), null, 2));
      toast.success("Diagnostics copied");
    } catch (copyError) {
      toast.error("Failed to copy diagnostics", {
        description: copyError instanceof Error ? copyError.message : String(copyError),
      });
    }
  };

  const saveDiagnosticsReport = async () => {
    if (!snapshot || loading || saveInProgress.current) return;
    saveInProgress.current = true;
    setSavingReport(true);
    try {
      if (await invoke<boolean>("save_diagnostics_report")) {
        toast.success("Diagnostics report saved");
      }
    } catch {
      toast.error("Could not save the diagnostics report", {
        description: "Choose a writable folder and a .json filename, then try again.",
      });
    } finally {
      saveInProgress.current = false;
      setSavingReport(false);
    }
  };

  const items = useMemo(() => {
    const list: HealthItem[] = [];

    const sourceNames = new Map(calendars.sources.map((source) => [source.id, source.name]));
    const broken = Object.entries(calendars.issues).filter(([id]) => sourceNames.has(id));
    if (broken.length > 0) {
      list.push({
        id: "calendars",
        name: "Calendars",
        value: broken.map(([id, issue]) => `${sourceNames.get(id)}: ${issue}`).join(" · "),
        tone: "warning",
        status: "Needs attention",
        action: (
          <Button type="button" variant="outline" size="sm" disabled={calendars.loading}
            onClick={() => { void refreshICalendars(true).catch(() => undefined); }}>
            {calendars.loading ? "Retrying…" : "Retry"}
          </Button>
        ),
      });
    } else if (calendars.sources.length > 0) {
      list.push({
        id: "calendars", name: "Calendars", tone: "ok", status: "OK",
        value: `${calendars.sources.length} ${calendars.sources.length === 1 ? "source" : "sources"}`,
      });
    }

    if (snapshot) {
      const acceleration = snapshot.acceleration;
      list.push({
        id: "backend", name: "Transcription backend",
        value: acceleration ? acceleration.label : "Unknown",
        tone: acceleration?.gpuAvailableButUnused ? "warning" : "ok",
        status: acceleration?.gpuAvailableButUnused ? "GPU not used" : "OK",
      });
      if (acceleration?.gpuAvailableButUnused) {
        list[list.length - 1].value = `${acceleration.runtimeDetectedGpu} GPU is visible, but this build uses CPU transcription`;
      }

      const engine = selectedEngine(snapshot);
      const modelName = `${providerLabel(snapshot.transcription.selectedProvider)} · ${getModelDisplayName(snapshot.transcription.selectedModel)}`;
      list.push({
        id: "model", name: "Selected model", value: modelName,
        tone: !engine ? "ok" : engine.error ? "warning" : engine.selectedModelPresent === false ? "error" : "ok",
        status: !engine ? "Unknown" : engine.error ? "Error" : engine.selectedModelPresent === false ? "Missing" : "Ready",
      });

      list.push({
        id: "audio", name: "Audio devices",
        value: snapshot.audio.error ?? `${snapshot.audio.inputDevices} input · ${snapshot.audio.outputDevices} output`,
        tone: snapshot.audio.error ? "error" : snapshot.audio.inputDevices > 0 ? "ok" : "warning",
        status: snapshot.audio.error ? "Error" : snapshot.audio.inputDevices > 0 ? "OK" : "No microphone",
      });

      const diarization = snapshot.diarization;
      const present = diarization.runtimeDlls.filter((dll) => dll.present).length;
      const slow = diarization.directmlSlow || diarization.directmlUnavailable;
      list.push({
        id: "diarization", name: "Speaker separation",
        value: diarization.inProgress
          ? "Running"
          : diarization.directmlSlow ? "DirectML probed slow, using the CPU"
          : diarization.directmlUnavailable ? "DirectML unavailable, using the CPU"
          : `${runtimeLabel(diarization.preferredProvider)} · ${present} of ${diarization.runtimeDlls.length} runtime files`,
        tone: slow ? "warning" : "ok",
        status: slow ? "CPU fallback" : "OK",
      });

      if (snapshot.errors.length > 0) {
        list.push({
          id: "errors", name: "Diagnostics data",
          value: `${snapshot.errors.length} ${snapshot.errors.length === 1 ? "problem" : "problems"} while collecting, see details`,
          tone: "warning", status: "Needs attention",
        });
      }
    }

    const claw = openclaw.status;
    list.push({
      id: "openclaw", name: "OpenClaw handoff",
      value: openclaw.error ? "Status unavailable" : claw ? claw.status_message : "Loading status…",
      tone: openclaw.error ? "warning" : claw && claw.enabled && !claw.ready ? "warning" : "ok",
      status: openclaw.error ? "Unavailable" : !claw ? "Checking" : claw.ready ? "Ready" : claw.enabled ? "Not ready" : "Off",
      action: <IconAction label="Refresh OpenClaw handoff status" onClick={() => void openclaw.load()} loading={openclaw.loading} />,
    });

    const cx = codex.status;
    list.push({
      id: "codex", name: "Codex app-server",
      value: codex.error ? "Status unavailable" : cx ? (cx.accountEmail ?? cx.message) : "Loading status…",
      tone: codex.error ? "warning" : "ok",
      status: codex.error ? "Unavailable" : !cx ? "Checking"
        : cx.authStatus === "authenticated" ? "Ready" : cx.found ? "Not signed in" : "Not available",
      action: <IconAction label="Refresh Codex status" onClick={() => void codex.load()} loading={codex.loading} />,
    });

    return sortHealthItems(list);
  }, [snapshot, calendars, openclaw.status, openclaw.error, openclaw.loading, openclaw.load, codex.status, codex.error, codex.loading, codex.load]);

  const buildLine = snapshot
    ? `ClawScribe ${snapshot.app.version} · ${snapshot.app.os} ${snapshot.app.arch} · ${snapshot.app.buildProfile}`
    : "Loading local diagnostics…";

  const health = (
    <SettingsCard
      title="Health"
      description={<Meta className="whitespace-normal">{buildLine}</Meta>}
      actions={
        <div className="flex items-center gap-2">
          <Button type="button" variant="outline" size="sm" onClick={refreshAll} disabled={loading}>
            <RefreshCw aria-hidden="true" className={loading ? "animate-spin" : ""} />Refresh
          </Button>
          <Button type="button" variant="outline" size="sm" onClick={copyDiagnostics} disabled={!snapshot}>
            <Copy aria-hidden="true" />Copy diagnostics
          </Button>
        </div>
      }
    >
      {error && <NoteRow tone="error">{error}</NoteRow>}
      {items.map((item) => <HealthRow key={item.id} item={item} />)}
      <DiagnosticsDetails snapshot={snapshot} openclaw={openclaw.status} />
      <SettingsFooter
        actions={
          <Button type="button" variant="outline" size="sm" onClick={saveDiagnosticsReport} disabled={!snapshot || loading || savingReport}>
            {savingReport ? "Saving…" : "Save report"}
          </Button>
        }
      >
        Save app and system information, status checks, and failure categories to a JSON file.
      </SettingsFooter>
    </SettingsCard>
  );

  return <SettingsColumns aside={<CallDetectionCard />}>{health}</SettingsColumns>;
}
