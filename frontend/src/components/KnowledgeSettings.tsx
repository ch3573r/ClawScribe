"use client";
import { useEffect, useRef, useState } from "react";
import { Info, MoreHorizontal } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Progress } from "@/components/ui/progress";
import { PageSection } from "@/components/ui/page-section";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { knowledgeService } from "@/services/knowledgeService";
import { libraryScope } from "@/lib/knowledge-state";
import type { IndexStatus, ModelStatus } from "@/types/knowledge";
const all = libraryScope([], true, { tags: [], mode: "any", untagged: false });

const indexReason: Record<string, string> = {
  disabled: "Semantic search is off.", model_unavailable: "Download the search model to prepare the index.",
  indexing_failed: "Some meetings could not be indexed. Retry to finish.", paused: "Indexing is paused.",
  indexing: "Preparing your meetings for semantic search.", busy: "Waiting for recording or other AI work to finish.",
};
export function KnowledgeSettings() {
  const [status, setStatus] = useState<IndexStatus | null>(null);
  const [model, setModel] = useState<ModelStatus | null>(null);
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
  const locked = !!busy || downloading;
  const bytes = (value: number) => `${(value / 1024 / 1024).toFixed(1)} MB`;
  const file = download?.current_file?.includes("tokenizer") ? "language files" : "search model";
  const percent = download ? Math.min(99, Math.floor(download.downloaded_bytes * 100 / Math.max(download.total_bytes, 1))) : 0;
  const retryModel = download?.stage === "error" || download?.stage === "cancelled";
  const downloadLabel = retryModel ? model?.installed ? "Retry verification" : "Retry download" : "Download search model";
  const modelStatus = model?.ready ? "Ready for semantic search." : model?.installed && !model.enabled ? "Search files are installed. Semantic search is off."
    : model?.installed ? "Search files are installed. Enable semantic search or verify the files." : model?.enabled ? "Download the search model to begin." : "Keyword search is available without a download.";
  const cancellationLabel = download?.stage === "verifying" || busy === "Verifying search files" ? "Cancel verification"
    : download?.stage === "checking" ? "Cancel check" : "Cancel download";
  return <TooltipProvider><div className="space-y-5">
    <PageSection title="Semantic search" actions={<div className="flex items-center gap-1">
      {model && <Tooltip><TooltipTrigger asChild><Button variant="ghost" size="icon" aria-label="Search model details"><Info /></Button></TooltipTrigger><TooltipContent>{model.model} · ONNX · CPU</TooltipContent></Tooltip>}
      <DropdownMenu><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" aria-label="Semantic search actions" disabled={!model || locked}><MoreHorizontal /></Button></DropdownMenuTrigger>
        <DropdownMenuContent align="end"><DropdownMenuItem disabled={!model?.installed} onSelect={() => void action("Verifying search files", knowledgeService.download)}>Verify installed files</DropdownMenuItem></DropdownMenuContent>
      </DropdownMenu>
    </div>}>
      <div className="space-y-4 p-5">
        <div className="flex items-start justify-between gap-4"><div>
          <label htmlFor="semantic-search-enabled" className="text-sm font-medium">Find passages by meaning</label>
          <p className="mt-1 text-sm text-muted-foreground">English and German, runs on this PC, about 465 MB download.</p>
        </div><Switch id="semantic-search-enabled" aria-label="Semantic search" checked={model?.enabled ?? false} disabled={!model || locked} onCheckedChange={enabled => void action("Updating semantic search", () => knowledgeService.enable(enabled))} /></div>
        {!model && !readError && <p role="status" className="text-sm text-muted-foreground">Loading search settings…</p>}
        {model && <p className="text-sm text-muted-foreground">{modelStatus}</p>}
        {model && (!model.installed || retryModel) && !downloading && <Button disabled={locked} onClick={() => void action(model.installed ? "Verifying search files" : "Downloading search model", knowledgeService.download)}>{downloadLabel}</Button>}
        {(downloading || busy === "Downloading search model" || busy === "Verifying search files") && <Button variant="outline" disabled={download?.stage === "cancelling"} onClick={() => void cancelDownload()}>{download?.stage === "cancelling" ? "Cancelling…" : cancellationLabel}</Button>}
        {downloading && download && <div role="status" className="space-y-2 text-sm text-muted-foreground">
          <p>{download.stage === "downloading" ? `Downloading ${file} · ${percent}% · ${bytes(download.downloaded_bytes)} / ${bytes(download.total_bytes)}`
            : download.stage === "verifying" ? `Verifying ${file}…` : download.stage === "cancelling" ? "Cancelling…" : `Checking ${file}…`}</p>
          {download.stage === "downloading" && <Progress aria-label="Model download progress" value={percent} aria-valuetext={`${bytes(download.downloaded_bytes)} of ${bytes(download.total_bytes)}`} />}
        </div>}
        {download?.stage === "error" && <p role="alert" className="text-sm text-destructive">Could not download or verify the search files. Check your connection and retry.</p>}
        {download?.stage === "cancelled" && <p role="status" className="text-sm text-muted-foreground">Download cancelled. Retry to continue from the saved progress.</p>}
      </div>
    </PageSection>
    <PageSection title="Index" actions={<DropdownMenu><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" aria-label="Index actions" disabled={!status || locked}><MoreHorizontal /></Button></DropdownMenuTrigger>
      <DropdownMenuContent align="end"><DropdownMenuItem disabled={!model?.ready || !status?.semantic_enabled} onSelect={() => void action("Rebuilding index", () => knowledgeService.reindex(all))}>Rebuild index</DropdownMenuItem></DropdownMenuContent>
    </DropdownMenu>}>
      <div className="space-y-4 p-5">
        {!status && !readError && <p role="status" className="text-sm text-muted-foreground">Loading index…</p>}
        {status && <><p className="text-sm">Keyword search is {status.keyword_ready ? "ready" : "preparing"}.</p>
          <p className="text-sm text-muted-foreground">Semantic search: {status.semantic_ready} ready · {status.pending} pending · {status.failed} failed</p>
          {model?.enabled && total > 0 && <Progress aria-label="Semantic index progress" value={status.semantic_ready * 100 / total} aria-valuetext={`${status.semantic_ready} of ${total} meetings ready`} />}
          {status.reason && <p role="status" className="text-sm text-muted-foreground">{indexReason[status.reason] ?? "Index status is unavailable. Trying again…"}</p>}
          <div className="flex flex-wrap gap-2">
            {status.pending > 0 && model?.ready && status.reason !== "paused" && <Button variant="outline" disabled={locked} onClick={() => void action("Pausing index", () => knowledgeService.pause(all))}>Pause</Button>}
            {status.failed > 0 && <Button variant="outline" disabled={locked || !model?.ready} onClick={() => void action("Retrying index", () => knowledgeService.reindex(all))}>Retry</Button>}
            {status.reason === "paused" && <Button variant="outline" disabled={locked || !model?.ready} onClick={() => void action("Resuming index", () => knowledgeService.reindex(all))}>Resume</Button>}
          </div>
        </>}
        <p className="text-xs text-muted-foreground">Recording and other AI work take priority. Rebuilding keeps saved citations.</p>
      </div>
    </PageSection>
    {busy && !downloading && <p role="status" className="text-sm text-muted-foreground">{busy}…</p>}
    {readError && <p role="alert" className="text-sm text-destructive">{readError}</p>}
    {actionError && <p role="alert" className="text-sm text-destructive">{actionError}</p>}
  </div></TooltipProvider>;
}
