"use client";
import { useEffect, useRef, useState } from "react";
import { Info, MoreHorizontal } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Progress } from "@/components/ui/progress";
import { Meta, SettingsCard, SettingsFooter, SettingsRow, SettingsStack, StatusText, type StatusTone } from "@/components/ui/settings-layout";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { knowledgeService } from "@/services/knowledgeService";
import { libraryScope } from "@/lib/knowledge-state";
import type { IndexStatus, ModelStatus } from "@/types/knowledge";
const all = libraryScope([], true, { tags: [], mode: "any", untagged: false });

const indexReason: Record<string, string> = {
  disabled: "Semantic search is off", model_unavailable: "The search model is unavailable",
  model_pending: "Indexing will verify the search files when needed", model_verifying: "Checking the search files",
  indexing_failed: "Some meetings could not be indexed. Retry to finish", paused: "Indexing is paused",
  indexing: "Preparing your meetings for semantic search", busy: "Waiting for recording or other AI work to finish",
};

/** Meeting memory: semantic search model, and the state of the search index. */
export function KnowledgeSettings() {
  const [status, setStatus] = useState<IndexStatus | null>(null);
  const [model, setModel] = useState<ModelStatus | null>(null);
  const [modelDetailsOpen, setModelDetailsOpen] = useState(false);
  const [busy, setBusy] = useState("");
  const [readError, setReadError] = useState("");
  const [actionError, setActionError] = useState("");
  const active = useRef(0);
  const pending = useRef(false);
  const reading = useRef(false);
  async function refresh() {
    if (reading.current) return;
    reading.current = true;
    const identity = active.current;
    try {
      const [index, configuration] = await Promise.all([knowledgeService.indexStatus(), knowledgeService.modelStatus()]);
      if (identity !== active.current) return;
      setStatus(index); setModel(configuration); setReadError("");
    } catch {
      if (identity === active.current) setReadError("Could not read meeting memory status. Trying again…");
    } finally { reading.current = false; }
  }
  useEffect(() => {
    active.current++;
    void refresh();
    const timer = setInterval(() => void refresh(), 1000);
    return () => { active.current++; clearInterval(timer); };
  }, []);
  async function action(label: string, operation: () => Promise<unknown>) {
    if (pending.current) return;
    pending.current = true; setBusy(label); setActionError("");
    const identity = active.current;
    try {
      await operation();
      if (identity !== active.current) return;
      await refresh();
    } catch (error) {
      if (identity === active.current) {
        setActionError(typeof error === "string" && /cancelled/i.test(error) ? "" : label === "Downloading search model"
          ? "Download failed. Check your connection and try again."
          : "Could not complete this action. Try again when recording and other AI work have finished.");
        await refresh();
      }
    } finally {
      if (identity === active.current) { pending.current = false; setBusy(""); }
    }
  }
  async function cancelDownload() {
    const identity = active.current;
    try { await knowledgeService.cancelDownload(); if (identity === active.current) await refresh(); }
    catch { if (identity === active.current) setActionError("Could not cancel the download. Try again."); }
  }
  const total = (status?.semantic_ready ?? 0) + (status?.pending ?? 0) + (status?.failed ?? 0);
  const download = model?.download;
  const downloading = !!download && ["checking", "downloading", "verifying", "cancelling"].includes(download.stage);
  const locked = !!busy || downloading || model?.verification === "updating";
  const canUseIndex = !!model?.enabled && model.installed && model.verification !== "unavailable";
  const bytes = (value: number) => `${(value / 1024 / 1024).toFixed(1)} MB`;
  const file = download?.current_file?.includes("tokenizer") ? "language files" : "search model";
  const percent = download ? Math.min(99, Math.floor(download.downloaded_bytes * 100 / Math.max(download.total_bytes, 1))) : 0;
  const retryModel = download?.stage === "error" || download?.stage === "cancelled";
  const downloadLabel = retryModel ? model?.installed ? "Retry verification" : "Retry download" : "Download search model";
  const modelStatus: { tone: StatusTone; text: string } = model?.ready ? { tone: "ok", text: "Ready for semantic search" }
    : model?.installed && !model.enabled ? { tone: "off", text: "Search files are installed. Semantic search is off" }
    : model?.verification === "verifying" || model?.verification === "updating" ? { tone: "neutral", text: "Checking search files…" }
    : model?.installed && model.verification === "unavailable" ? { tone: "warning", text: "Could not use the search model. Verify the installed files to try again" }
    : model?.installed ? { tone: "neutral", text: "Search files will be verified on first use" }
    : model?.enabled ? { tone: "warning", text: "Download the search model to begin" } : { tone: "neutral", text: "Keyword search is available without a download" };
  const cancellationLabel = download?.stage === "verifying" || busy === "Verifying search files" ? "Cancel verification"
    : download?.stage === "checking" ? "Cancel check" : "Cancel download";
  const showCancel = downloading || busy === "Downloading search model" || busy === "Verifying search files";
  return <SettingsStack>
    <SettingsCard>
      <SettingsRow title="Find passages by meaning" htmlFor="semantic-search-enabled" description="English and German, runs on this PC, 465 MB model">
        <Switch id="semantic-search-enabled" aria-label="Semantic search" checked={model?.enabled ?? false} disabled={!model || locked}
          onCheckedChange={enabled => void action("Updating semantic search", () => knowledgeService.enable(enabled))} />
      </SettingsRow>
      {downloading && download && <SettingsRow role="status" title={download.stage === "downloading" ? `Downloading ${file} · ${percent}% · ${bytes(download.downloaded_bytes)} / ${bytes(download.total_bytes)}`
        : download.stage === "verifying" ? `Verifying ${file}…` : download.stage === "cancelling" ? "Cancelling…" : `Checking ${file}…`}>
        {download.stage === "downloading" && <Progress className="w-48" aria-label="Model download progress" value={percent} aria-valuetext={`${bytes(download.downloaded_bytes)} of ${bytes(download.total_bytes)}`} />}
      </SettingsRow>}
      {download?.stage === "error" && <p role="alert" className="px-5 py-3 text-[13px] text-error-foreground">Could not download or verify the search files. Check your connection and retry.</p>}
      {download?.stage === "cancelled" && <p role="status" className="px-5 py-3 text-[13px] text-muted-foreground">Download cancelled. Retry to continue from the saved progress.</p>}
      <SettingsFooter actions={<>
        {model && <Popover open={modelDetailsOpen} onOpenChange={setModelDetailsOpen}>
          <PopoverTrigger asChild><Button type="button" variant="ghost" size="icon" aria-label="Search model details"><Info /></Button></PopoverTrigger>
          <PopoverContent align="end" aria-label="Search model details" className="space-y-2 text-sm">
            <p className="break-words">{model.model} · ONNX · CPU</p>
            <p className="text-muted-foreground">Choose Semantic + keyword in Search mode under Meetings → Meeting memory → Source search.</p>
          </PopoverContent>
        </Popover>}
        {model?.installed && !downloading && !retryModel && <Button type="button" variant="ghost" size="sm" disabled={locked} onClick={() => void action("Verifying search files", knowledgeService.download)}>Verify installed files</Button>}
        {showCancel && <Button variant="outline" size="sm" disabled={download?.stage === "cancelling"} onClick={() => void cancelDownload()}>{download?.stage === "cancelling" ? "Cancelling…" : cancellationLabel}</Button>}
        {model && (!model.installed || retryModel) && !downloading && <Button size="sm" disabled={locked} onClick={() => void action(model.installed ? "Verifying search files" : "Downloading search model", knowledgeService.download)}>{downloadLabel}</Button>}
      </>}>
        {!model && !readError && <span role="status">Loading search settings…</span>}
        {model && <span role="status"><StatusText tone={modelStatus.tone}>{modelStatus.text}</StatusText></span>}
        {busy && !downloading && <span role="status">{busy}…</span>}
      </SettingsFooter>
    </SettingsCard>
    <SettingsCard title="Index" actions={canUseIndex && status?.semantic_enabled && total > 0 ? <DropdownMenu><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" aria-label="Index actions" disabled={locked}><MoreHorizontal /></Button></DropdownMenuTrigger>
      <DropdownMenuContent align="end"><DropdownMenuItem disabled={locked} onSelect={() => void action("Rebuilding index", () => knowledgeService.reindex(all))}>Rebuild index</DropdownMenuItem></DropdownMenuContent>
    </DropdownMenu> : null}>
      {!status && !readError && <SettingsRow title="Loading index…" role="status" />}
      {status && <>
        <SettingsRow title="Keyword search"><StatusText tone={status.keyword_ready ? "ok" : "neutral"}>{status.keyword_ready ? "Ready" : "Preparing"}</StatusText></SettingsRow>
        <SettingsRow title="Semantic search" description={status.reason ? <span role="status">{indexReason[status.reason] ?? "Index status is unavailable. Trying again…"}</span> : undefined}>
          {model?.enabled && total > 0
            ? <>
              <Progress className="w-48" aria-label="Semantic index progress" value={status.semantic_ready * 100 / total} aria-valuetext={`${status.semantic_ready} of ${total} meetings ready`} />
              <Meta>{status.semantic_ready} of {total} meetings · {status.pending > 0 ? `${status.pending} pending · ` : ""}{status.failed} failed</Meta>
            </>
            : model?.enabled ? <Meta>{status.semantic_ready} ready · {status.pending} pending · {status.failed} failed</Meta> : <StatusText tone="off">Off</StatusText>}
          {status.pending > 0 && canUseIndex && status.reason !== "paused" && <Button variant="outline" size="sm" disabled={locked} onClick={() => void action("Pausing index", () => knowledgeService.pause(all))}>Pause</Button>}
          {status.failed > 0 && <Button variant="outline" size="sm" disabled={locked || !canUseIndex} onClick={() => void action("Retrying index", () => knowledgeService.reindex(all))}>Retry</Button>}
          {status.reason === "paused" && <Button variant="outline" size="sm" disabled={locked || !canUseIndex} onClick={() => void action("Resuming index", () => knowledgeService.reindex(all))}>Resume</Button>}
        </SettingsRow>
      </>}
      <SettingsFooter>Recording and other AI work take priority over indexing. Rebuilding keeps saved citations</SettingsFooter>
    </SettingsCard>
    {readError && <p role="alert" className="text-sm text-error-foreground">{readError}</p>}
    {actionError && <p role="alert" className="text-sm text-error-foreground">{actionError}</p>}
  </SettingsStack>;
}
