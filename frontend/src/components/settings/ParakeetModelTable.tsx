"use client";

import type { ReactNode } from "react";
import { Download, MoreHorizontal } from "lucide-react";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Progress } from "@/components/ui/progress";
import { Meta, SettingsCard, SettingsFooter, SettingsRow, SettingsTag, StatusText } from "@/components/ui/settings-layout";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { formatFileSize, formatModelSize, getModelDisplayInfo, type ParakeetModelInfo } from "@/lib/parakeet";
import { fastestSpeed, formatMeasuredAt, formatSpeed, speedFraction, speedOf, type PcTestRecord } from "@/components/settings/pcTestResults";

interface ParakeetModelTableProps {
  models: ParakeetModelInfo[];
  loading: boolean;
  error: string | null;
  selectedModel?: string;
  /** Measured speed per model id. */
  results: Record<string, PcTestRecord>;
  /** The "Test this PC" control for the card header. Omitted where testing is not offered. */
  testAction?: ReactNode;
  /** Outcome of the latest test, shown under the table. */
  testReport?: ReactNode;
  onSelect: (modelName: string) => void;
  onDownload: (modelName: string) => void;
  onCancel: (modelName: string) => void;
  onDelete: (modelName: string) => void;
  onRetryLoad: () => void;
}

const COLUMNS = ["Model", "Best for", "Speed on this PC", "Download", "Status", ""];

function statusOf(status: ParakeetModelInfo["status"]) {
  if (status === "Available") return { kind: "available" as const };
  if (status === "Missing") return { kind: "missing" as const };
  if (typeof status === "object" && "Downloading" in status) return { kind: "downloading" as const, progress: status.Downloading };
  if (typeof status === "object" && "Error" in status) return { kind: "error" as const, message: status.Error };
  return { kind: "corrupted" as const };
}

function SpeedCell({ record, fastest, current }: { record: PcTestRecord | undefined; fastest: number; current: boolean }) {
  if (!record) return <Meta>Not tested</Meta>;
  const speed = speedOf(record);
  const detail = `${record.backend ? `Measured on ${record.backend}` : "Measured"} ${formatMeasuredAt(record.measuredAt)}`;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <div tabIndex={0} className="flex items-center gap-3 rounded focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
          <span
            aria-hidden="true"
            className={`hidden h-1.5 shrink-0 rounded-full min-[1500px]:block ${current ? "bg-primary" : "bg-muted-foreground/40"}`}
            style={{ width: `${speedFraction(speed, fastest) * 8}rem` }}
          />
          <div>
            <Meta>{formatSpeed(speed)}</Meta>
            {record.missedSpeech && <span className="block text-xs text-warning-foreground">Missed speech</span>}
          </div>
        </div>
      </TooltipTrigger>
      <TooltipContent>{detail}</TooltipContent>
    </Tooltip>
  );
}

function RowActions({ model, inUse, onSelect, onDownload, onCancel, onDelete }: {
  model: ParakeetModelInfo; inUse: boolean;
} & Pick<ParakeetModelTableProps, "onSelect" | "onDownload" | "onCancel" | "onDelete">) {
  const name = getModelDisplayInfo(model.name)?.friendlyName || model.name;
  const status = statusOf(model.status);
  switch (status.kind) {
    case "available":
      return (
        <div className="flex items-center justify-end gap-1">
          {!inUse && <Button variant="outline" size="sm" aria-label={`Use ${name}`} onClick={() => onSelect(model.name)}>Use</Button>}
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon" className="h-8 w-8" aria-label={`More actions for ${name}`}>
                <MoreHorizontal aria-hidden="true" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onSelect={() => onDelete(model.name)}>Delete model to free up space</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      );
    case "missing":
      return (
        <Button variant="outline" size="sm" aria-label={`Download ${name}`} onClick={() => onDownload(model.name)}>
          <Download aria-hidden="true" />Download
        </Button>
      );
    case "downloading":
      return <Button variant="ghost" size="sm" aria-label={`Cancel download of ${name}`} onClick={() => onCancel(model.name)}>Cancel</Button>;
    case "error":
      return <Button variant="outline" size="sm" aria-label={`Retry download of ${name}`} onClick={() => onDownload(model.name)}>Retry</Button>;
    case "corrupted":
      return (
        <div className="flex items-center justify-end gap-1">
          <Button variant="ghost" size="sm" aria-label={`Delete ${name}`} onClick={() => onDelete(model.name)}>Delete</Button>
          <Button variant="outline" size="sm" aria-label={`Download ${name} again`} onClick={() => onDownload(model.name)}>Re-download</Button>
        </div>
      );
  }
}

function StatusCell({ model, inUse }: { model: ParakeetModelInfo; inUse: boolean }) {
  const status = statusOf(model.status);
  switch (status.kind) {
    case "available":
      return inUse ? <StatusText tone="active">In use</StatusText> : <StatusText tone="ok">Ready</StatusText>;
    case "missing":
      return <StatusText tone="off">Not downloaded</StatusText>;
    case "downloading":
      return (
        <div className="grid min-w-28 gap-1.5">
          <StatusText tone="active">Downloading {Math.round(status.progress)}%</StatusText>
          <Progress value={status.progress} className="h-1.5" aria-label={`Download progress, ${Math.round(status.progress)}%`} />
        </div>
      );
    case "error":
      return (
        <div className="grid gap-1">
          <StatusText tone="error">Download failed</StatusText>
          <span className="max-w-64 whitespace-normal break-words text-xs text-muted-foreground">{status.message}</span>
        </div>
      );
    case "corrupted":
      return <StatusText tone="warning">Incomplete download</StatusText>;
  }
}

/** Every Parakeet model in one table: speed on this PC, size, state and the action that applies. */
export function ParakeetModelTable({
  models, loading, error, selectedModel, results, testAction, testReport,
  onSelect, onDownload, onCancel, onDelete, onRetryLoad,
}: ParakeetModelTableProps) {
  const recommended = models.filter(model => getModelDisplayInfo(model.name)?.recommended);
  const ordered = [...recommended, ...models.filter(model => !recommended.includes(model))];
  const fastest = fastestSpeed(ordered.flatMap(model => (results[model.name] ? [results[model.name]] : [])));
  const selectedSupported = !selectedModel || models.some(model => model.name === selectedModel);

  return (
    <SettingsCard
      title="Models"
      description="Speed is measured on this PC with 30 seconds of German and English speech"
      aria-busy={loading}
      actions={testAction ? <div className="flex shrink-0 items-center gap-3">{testAction}</div> : undefined}
    >
      {loading ? (
        <SettingsRow title="Loading models" role="status" />
      ) : error ? (
        <SettingsRow title="Models unavailable" description={error}>
          <Button variant="outline" size="sm" onClick={onRetryLoad}>Try again</Button>
        </SettingsRow>
      ) : ordered.length === 0 ? (
        <SettingsRow title="No models available" description="This build offers no Parakeet models" />
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full border-collapse text-sm">
            <thead>
              <tr className="border-b border-border">
                {COLUMNS.map((column, index) => (
                  <th
                    key={index}
                    scope="col"
                    className={`px-4 py-2.5 text-left text-xs font-normal text-muted-foreground first:pl-5 last:pr-5 ${column === "Best for" || column === "Download" ? "max-[1499px]:hidden" : ""}`}
                  >
                    {column || <span className="sr-only">Action</span>}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {ordered.map(model => {
                const info = getModelDisplayInfo(model.name);
                const name = info?.friendlyName || model.name;
                const bestFor = info?.bestFor || model.description || "";
                const inUse = selectedModel === model.name && model.status === "Available";
                const status = statusOf(model.status);
                return (
                  <tr key={model.name} className={`border-t border-border/60 first:border-t-0 ${inUse ? "bg-primary/10" : ""}`}>
                    <td className="relative px-4 py-3.5 pl-5">
                      {inUse && <span aria-hidden="true" className="absolute inset-y-0 left-0 w-[3px] bg-primary" />}
                      <span className="whitespace-nowrap text-foreground">{name}</span>
                      {info?.badge && <SettingsTag>{info.badge}</SettingsTag>}
                      <span className="block text-[13px] text-muted-foreground min-[1500px]:hidden">{bestFor} · {formatModelSize(model.size_mb)}</span>
                    </td>
                    <td className="px-4 py-3.5 text-muted-foreground max-[1499px]:hidden">{bestFor}</td>
                    <td className="px-4 py-3.5">
                      <SpeedCell record={results[model.name]} fastest={fastest} current={inUse} />
                    </td>
                    <td className="px-4 py-3.5 max-[1499px]:hidden">
                      {status.kind === "downloading" && model.size_mb
                        ? <Meta>{formatFileSize(Math.round(model.size_mb * status.progress / 100))} / {formatFileSize(model.size_mb)}</Meta>
                        : <Meta>{formatModelSize(model.size_mb)}</Meta>}
                    </td>
                    <td className="px-4 py-3.5"><StatusCell model={model} inUse={inUse} /></td>
                    <td className="px-4 py-3.5 pr-5 text-right">
                      <RowActions model={model} inUse={inUse} onSelect={onSelect} onDownload={onDownload} onCancel={onCancel} onDelete={onDelete} />
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
      {!loading && !error && !selectedSupported && (
        <SettingsFooter>
          <StatusText tone="warning" className="whitespace-normal">The selected Parakeet model is no longer supported. Choose another model from the table</StatusText>
        </SettingsFooter>
      )}
      {testReport}
    </SettingsCard>
  );
}
