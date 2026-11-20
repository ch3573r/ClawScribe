"use client";

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { HardDrive } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from "@/components/ui/dialog";
import { formatArchiveBytes } from "@/lib/archive-review";

interface ArchiveStorage {
  saved_audio_bytes: number;
  recovery_bytes: number;
  database_bytes: number;
  reference_bytes: number;
  database_backup_bytes: number;
  unavailable_folders: number;
  partial: boolean;
}

const classes = {
  saved_audio_bytes: "Audio in saved meeting folders",
  recovery_bytes: "Recovery files in saved meeting folders",
  database_bytes: "Library database and indexes",
  reference_bytes: "Reference originals",
  database_backup_bytes: "Local database backups",
} as const;

export function ArchiveStorageButton() {
  const [open, setOpen] = useState(false);
  const [revision, setRevision] = useState(0);
  const [storage, setStorage] = useState<ArchiveStorage | null>(null);
  const [error, setError] = useState(false);

  useEffect(() => {
    if (!open) return;
    let current = true;
    setStorage(null);
    setError(false);
    void invoke<ArchiveStorage>("measure_archive_storage").then(
      result => { if (current) setStorage(result); },
      () => { if (current) setError(true); },
    );
    return () => { current = false; };
  }, [open, revision]);

  return <>
    <Button variant="outline" onClick={() => setOpen(true)}><HardDrive aria-hidden="true" />Storage</Button>
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Library storage</DialogTitle>
          <DialogDescription>File sizes for this library. Backup ZIPs and model downloads are outside these totals.</DialogDescription>
        </DialogHeader>
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
        <DialogFooter>
          <Button variant="outline" disabled={!error && !storage} onClick={() => setRevision(current => current + 1)}>{error ? "Retry" : "Measure again"}</Button>
          <Button onClick={() => setOpen(false)}>Done</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </>;
}
