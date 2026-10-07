"use client";
import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { knowledgeService } from "@/services/knowledgeService";
import { libraryScope } from "@/lib/knowledge-state";
import type { IndexStatus, ModelStatus } from "@/types/knowledge";
const all = libraryScope([], true, { tags: [], mode: "any", untagged: false });
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
      const [index, configuration] = await Promise.all([
        knowledgeService.indexStatus(),
        knowledgeService.modelStatus(),
      ]);
      if (identity !== active.current) return;
      setStatus(index);
      setModel(configuration);
      setReadError("");
    } catch {
      if (identity === active.current)
        setReadError(
          "Could not read meeting memory status. Retry to reconnect.",
        );
    } finally {
      reading.current = false;
    }
  }
  useEffect(() => {
    active.current++;
    void refresh();
    const timer = setInterval(() => void refresh(), 2500);
    return () => {
      active.current++;
      clearInterval(timer);
    };
  }, []);
  async function action(label: string, operation: () => Promise<unknown>) {
    if (pending.current) return;
    pending.current = true;
    setBusy(label);
    setActionError("");
    const identity = active.current;
    try {
      await operation();
      if (identity !== active.current) return;
      await refresh();
      if (identity !== active.current) return;
    } catch (e) {
      if (identity === active.current)
        setActionError(
          typeof e === "string"
            ? e
            : "Meeting memory operation failed. Retry when recording and other inference work have stopped.",
        );
    } finally {
      if (identity === active.current) {
        pending.current = false;
        setBusy("");
      }
    }
  }
  async function cancelDownload() {
    const identity = active.current;
    try {
      await knowledgeService.cancelDownload();
      if (identity !== active.current) return;
    } catch {
      if (identity === active.current)
        setActionError("Could not cancel model download. Retry.");
    }
  }
  const total =
    (status?.semantic_ready ?? 0) +
    (status?.pending ?? 0) +
    (status?.failed ?? 0);
  return (
    <section className="space-y-5 rounded-lg border border-border bg-card p-5 text-foreground">
      <div>
        <h2 className="text-lg font-semibold">Meeting memory</h2>
        <p className="mt-1 text-sm text-muted-foreground">
          Keyword search stays local and works immediately. Enable the optional
          local embedding model for semantic retrieval.
        </p>
      </div>
      {model && (
        <div className="space-y-2">
          <p className="text-sm">
            Embedding model: <span className="font-medium">{model.model}</span>
          </p>
          <p className="text-xs text-muted-foreground">
            {model.ready
              ? "Ready for local semantic retrieval"
              : model.enabled
                ? "Enabled; download or retry initialization to become ready"
                : "Semantic indexing is off"}
          </p>
          <Button
            variant="outline"
            disabled={!!busy}
            onClick={() =>
              void action("Updating model", () =>
                knowledgeService.enable(!model.enabled),
              )
            }
          >
            {model.enabled
              ? "Disable semantic indexing"
              : "Enable semantic indexing"}
          </Button>
        </div>
      )}
      <div className="flex flex-wrap gap-2">
        <Button
          disabled={!!busy}
          onClick={() =>
            void action(
              "Downloading embedding model",
              knowledgeService.download,
            )
          }
        >
          Download / repair model
        </Button>
        {busy === "Downloading embedding model" && (
          <Button variant="outline" onClick={() => void cancelDownload()}>
            Cancel download
          </Button>
        )}
      </div>
      {status && (
        <div className="space-y-2">
          <p className="text-sm">
            Keyword index: {status.keyword_ready ? "ready" : "not ready"} ·
            Semantic index: {status.semantic_ready} ready, {status.pending}{" "}
            pending, {status.failed} failed
          </p>
          <progress
            aria-label="Semantic index progress"
            value={status.semantic_ready}
            max={Math.max(total, 1)}
            className="w-full accent-primary"
          />
          {status.reason && (
            <p className="text-xs text-muted-foreground">{status.reason}</p>
          )}
        </div>
      )}
      <div className="flex flex-wrap gap-2">
        <Button
          variant="outline"
          disabled={!!busy}
          onClick={() =>
            void action("Pausing index", () => knowledgeService.pause(all))
          }
        >
          Pause
        </Button>
        <Button
          variant="outline"
          disabled={!!busy}
          onClick={() =>
            void action("Retrying index", () => knowledgeService.reindex(all))
          }
        >
          Retry / resume
        </Button>
        <Button
          variant="outline"
          disabled={!!busy}
          onClick={() =>
            void action("Rebuilding index", () => knowledgeService.reindex(all))
          }
        >
          Rebuild
        </Button>
        <Button
          variant="ghost"
          disabled={!!busy}
          onClick={() => void refresh()}
        >
          Refresh status
        </Button>
      </div>
      {busy && (
        <p role="status" className="text-sm text-muted-foreground">
          {busy}…
        </p>
      )}
      {readError && (
        <p role="alert" className="text-sm text-destructive">
          {readError}
        </p>
      )}
      {actionError && (
        <p role="alert" className="text-sm text-destructive">
          {actionError}
        </p>
      )}
      <p className="text-xs text-muted-foreground">
        Recording and foreground inference take priority. Saved citations
        survive index rebuilds. Answer questions with the provider configured
        under Summary.
      </p>
    </section>
  );
}
