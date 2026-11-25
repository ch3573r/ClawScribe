"use client";

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from "@/components/ui/dialog";
import { formatArchiveBytes } from "@/lib/archive-review";
import { registerStorageDialogOpener } from "@/lib/storage-dialog-controller";

interface ArchiveStorage {
  saved_audio_bytes: number;
  recovery_bytes: number;
  database_bytes: number;
  reference_bytes: number;
  database_backup_bytes: number;
  unavailable_folders: number;
  partial: boolean;
}

interface PendingFileCleanup {
  id: number;
  title: string;
  kind: string;
  location: string | null;
  reason: string | null;
}

const cleanupTypes: Record<string, string> = {
  recording: "Recording folder",
  reference: "Reference original",
  outputs: "Generated meeting outputs",
  codex: "Codex scratch files",
  exports: "Export history",
};

const classes = {
  saved_audio_bytes: "Audio in saved meeting folders",
  recovery_bytes: "Recovery files in saved meeting folders",
  database_bytes: "Library database and indexes",
  reference_bytes: "Reference originals",
  database_backup_bytes: "Local database backups",
} as const;

interface MeetingStoragePage {
  rows: {
    title: string;
    audio_bytes: number;
    recovery_bytes: number;
    other_bytes: number;
    no_folder: boolean;
    unavailable: boolean;
    partial: boolean;
  }[];
  next_cursor: string | null;
}

function MeetingStorageBreakdown() {
  const [cursors, setCursors] = useState<(string | null)[]>([null]);
  const [result, setResult] = useState<MeetingStoragePage | null>(null);
  const [revision, setRevision] = useState(0);
  const [error, setError] = useState(false);
  const cursor = cursors[cursors.length - 1];
  useEffect(() => {
    let active = true;
    setResult(null);
    setError(false);
    void invoke<MeetingStoragePage>("measure_meeting_storage_page", { cursor }).then(
      page => { if (active) setResult(page); },
      () => { if (active) setError(true); },
    );
    return () => { active = false; };
  }, [cursor, revision]);
  return <div className="space-y-2">
    <p className="text-sm text-muted-foreground">Recording folders only. Shared database and reference files are listed above.</p>
    {error ? <p role="alert" className="text-sm text-destructive">Could not measure this page.</p>
      : !result ? <p role="status" className="text-sm text-muted-foreground">Measuring meeting folders…</p>
        : result.rows.length === 0 ? <p className="text-sm text-muted-foreground">No meetings on this page.</p>
          : <ul aria-label="Storage used by meeting" className="divide-y divide-border">
            {result.rows.map((row, index) => <li key={index} className="py-2 text-sm">
              <p className="break-words font-medium">{row.title || "Untitled meeting"}</p>
              <p className="text-muted-foreground">
                {row.no_folder ? "No recording folder" : row.unavailable ? "Folder unavailable" : [
                  `Audio ${formatArchiveBytes(row.audio_bytes)}`,
                  `Recovery ${formatArchiveBytes(row.recovery_bytes)}`,
                  `Other files ${formatArchiveBytes(row.other_bytes)}`,
                  row.partial && "Partial measurement",
                ].filter(Boolean).join(" · ")}
              </p>
            </li>)}
          </ul>}
    <div className="flex items-center justify-between gap-2">
      <Button type="button" variant="ghost" size="sm" disabled={cursors.length === 1} onClick={() => { setResult(null); setCursors(current => current.slice(0, -1)); }}>Previous</Button>
      <span aria-live="polite" className="text-sm text-muted-foreground">Page {cursors.length}</span>
      {error ? <Button type="button" size="sm" variant="outline" onClick={() => { setError(false); setRevision(value => value + 1); }}>Retry</Button>
        : <Button type="button" variant="ghost" size="sm" disabled={!result?.next_cursor} onClick={() => {
          if (result?.next_cursor) { const next = result.next_cursor; setResult(null); setCursors(current => [...current, next]); }
        }}>Next</Button>}
    </div>
  </div>;
}

export function ArchiveStorageDialog() {
  const [open, setOpen] = useState(false);
  const [revision, setRevision] = useState(0);
  const [storage, setStorage] = useState<ArchiveStorage | null>(null);
  const [error, setError] = useState(false);
  const [cleanup, setCleanup] = useState<PendingFileCleanup[] | null>(null);
  const [cleanupError, setCleanupError] = useState(false);
  const [review, setReview] = useState<PendingFileCleanup | null>(null);
  const [working, setWorking] = useState(false);
  const [status, setStatus] = useState("");
  const [showBreakdown, setShowBreakdown] = useState(false);
  const [actionError, setActionError] = useState("");
  const mutation = useRef(false);
  const returnFocus = useRef<HTMLElement | null>(null);
  const title = useRef<HTMLHeadingElement | null>(null);
  const reviewing = review !== null;

  useEffect(() => registerStorageDialogOpener(() => {
    const focused = document.activeElement;
    returnFocus.current = focused && focused instanceof HTMLElement ? focused : null;
    setOpen(true);
  }), []);

  useEffect(() => { if (open) title.current?.focus(); }, [open, reviewing]);

  useEffect(() => {
    if (!open) return;
    let current = true;
    setStorage(null);
    setError(false);
    setCleanup(null);
    setCleanupError(false);
    void invoke<ArchiveStorage>("measure_archive_storage").then(
      result => { if (current) setStorage(result); },
      () => { if (current) setError(true); },
    );
    void invoke<PendingFileCleanup[]>("list_pending_meeting_file_cleanup").then(
      result => { if (current) setCleanup(result); },
      () => { if (current) setCleanupError(true); },
    );
    return () => { current = false; };
  }, [open, revision]);

  const changeOpen = (next: boolean) => {
    if (mutation.current) return;
    setOpen(next);
    if (!next) { setReview(null); setActionError(""); setStatus(""); setShowBreakdown(false); }
  };

  const actOnCleanup = async (keep: boolean) => {
    if (!review || mutation.current) return;
    mutation.current = true;
    setWorking(true);
    setActionError("");
    try {
      const args = { id: review.id };
      const reason = keep
        ? await invoke<void>("keep_pending_meeting_files", args)
        : await invoke<string | null>("retry_meeting_file_cleanup", args);
      if (reason) {
        setReview({ ...review, reason });
        setCleanup(items => items?.map(item => item.id === review.id ? { ...item, reason } : item) ?? null);
        setActionError("The cleanup could not finish.");
      } else {
        setReview(null);
        setStatus(keep ? "Files kept and removed from the cleanup list." : "File cleanup complete.");
        setRevision(value => value + 1);
      }
    } catch {
      setActionError("Could not update this cleanup entry. Try again.");
    } finally {
      mutation.current = false;
      setWorking(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={changeOpen}>
      <DialogContent className="max-h-[85vh] overflow-y-auto" onCloseAutoFocus={event => {
        if (returnFocus.current?.isConnected) {
          event.preventDefault();
          returnFocus.current.focus();
        }
        returnFocus.current = null;
      }}>
        <DialogHeader>
          <DialogTitle ref={title} tabIndex={-1}>{review ? "Review pending file cleanup" : "Library storage"}</DialogTitle>
          <DialogDescription>{review ? "Retrying permanently removes these files if ClawScribe can still verify ownership." : "File sizes for this library. Backup ZIPs and model downloads are outside these totals."}</DialogDescription>
        </DialogHeader>
        {review ? <>
          <div className="space-y-2 text-sm">
            <p className="break-words font-medium">{review.title || "Untitled meeting"} · {cleanupTypes[review.kind] ?? "Meeting files"}</p>
            {review.location && <p className="break-all text-muted-foreground">{review.location}</p>}
            <p>{review.reason ?? "The previous cleanup did not finish."}</p>
            {working && <p role="status">Updating file cleanup…</p>}
            {actionError && <p role="alert" className="text-destructive">{actionError}</p>}
          </div>
          <DialogFooter>
            <Button variant="ghost" disabled={working} onClick={() => { setReview(null); setActionError(""); }}>Back to storage</Button>
            <Button variant="outline" disabled={working} onClick={() => void actOnCleanup(true)}>Keep files</Button>
            <Button disabled={working} onClick={() => void actOnCleanup(false)}>Retry cleanup</Button>
          </DialogFooter>
        </> : <>
          {error ? <p role="alert">Could not measure library storage.</p> : storage ? <>
          <dl className="divide-y divide-border text-sm">
            {(Object.keys(classes) as (keyof typeof classes)[]).map(key => (
              <div key={key} className="flex items-baseline justify-between gap-4 py-2">
                <dt>{classes[key]}</dt><dd className="shrink-0 font-medium tabular-nums">{formatArchiveBytes(storage[key])}</dd>
              </div>
            ))}
          </dl>
          {(storage.partial || storage.unavailable_folders > 0) && (
            <p role="status" className="text-sm text-warning-foreground">
              Some files or recording locations could not be measured. The totals cover available files.
            </p>
          )}
          </> : <p role="status">Measuring library storage…</p>}
          <section aria-label="Storage by meeting" className="space-y-2">
            <Button type="button" variant="outline" size="sm" aria-expanded={showBreakdown} aria-controls="storage-by-meeting"
              onClick={() => setShowBreakdown(value => !value)}>
              {showBreakdown ? "Hide storage by meeting" : "Show storage by meeting"}
            </Button>
            <div id="storage-by-meeting" hidden={!showBreakdown}>
              {showBreakdown && <MeetingStorageBreakdown key={revision} />}
            </div>
          </section>
          {status && <p role="status" className="text-sm">{status}</p>}
          {cleanupError ? <p role="alert">Could not load pending file cleanup.</p> : cleanup === null ? <p role="status">Checking for pending file cleanup…</p> : cleanup.length > 0 && (
            <section aria-label="Pending file cleanup" className="space-y-2">
              <h3 className="font-medium">Pending file cleanup</h3>
              <ul className="divide-y divide-border text-sm">
                {cleanup.slice(0, 50).map(item => <li key={item.id} className="flex items-center justify-between gap-4 py-2">
                  <p className="min-w-0 break-words">{item.title || "Untitled meeting"}<span className="block text-muted-foreground">{cleanupTypes[item.kind] ?? "Meeting files"}</span></p>
                  <Button variant="outline" size="sm" aria-label={`Review ${cleanupTypes[item.kind] ?? "meeting files"} for ${item.title || "untitled meeting"}`} onClick={() => { setReview(item); setStatus(""); }}>Review</Button>
                </li>)}
              </ul>
              {cleanup.length > 50 && <p className="text-sm text-muted-foreground">Showing the 50 most recent entries. Remaining entries appear as you resolve these.</p>}
            </section>
          )}
          <DialogFooter>
            <Button variant="outline" disabled={!error && !storage && !cleanupError} onClick={() => setRevision(current => current + 1)}>{error || cleanupError ? "Retry" : "Measure again"}</Button>
            <Button onClick={() => changeOpen(false)}>Done</Button>
          </DialogFooter>
        </>}
      </DialogContent>
    </Dialog>
  );
}
