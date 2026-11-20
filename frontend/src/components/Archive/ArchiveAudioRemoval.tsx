"use client";

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from "@/components/ui/dialog";
import { formatArchiveBytes } from "@/lib/archive-review";

export interface ArchiveMeeting {
  id: string;
  title: string;
  created_at?: string;
}

interface RemovalCheck {
  id: string;
  bytes: number | null;
  revision: string | null;
  reason: string | null;
}

interface RemovalResult {
  id: string;
  removed_bytes: number | null;
  reason: string | null;
}

export function ArchiveAudioRemoval({ meetings, onClose, onRemoved }: {
  meetings: readonly ArchiveMeeting[];
  onClose: () => void;
  onRemoved: (ids: string[]) => void;
}) {
  const [checks, setChecks] = useState<RemovalCheck[] | null>(null);
  const [results, setResults] = useState<RemovalResult[] | null>(null);
  const [error, setError] = useState("");
  const [working, setWorking] = useState(false);
  const [revision, setRevision] = useState(0);
  const operation = useRef(false);
  const dismissButton = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    let current = true;
    setChecks(null);
    setError("");
    void invoke<RemovalCheck[]>("inspect_archive_audio", { ids: meetings.map(meeting => meeting.id) }).then(
      value => { if (current) setChecks(value); },
      () => { if (current) setError("Could not check the saved audio."); },
    );
    return () => { current = false; };
  }, [meetings, revision]);

  const eligible = (checks ?? []).filter(check => check.revision !== null);
  const bytes = eligible.reduce((sum, check) => sum + (check.bytes ?? 0), 0);
  const title = new Map(meetings.map(meeting => [meeting.id, meeting.title]));

  async function remove() {
    if (operation.current || !eligible.length) return;
    operation.current = true;
    setWorking(true);
    setError("");
    try {
      const value = await invoke<RemovalResult[]>("remove_archive_audio", {
        requests: eligible.map(({ id, revision }) => ({ id, revision })),
      });
      setResults(value);
      onRemoved(value.filter(result => result.removed_bytes !== null).map(result => result.id));
    } catch (failure) {
      setError(typeof failure === "string" ? failure : "Could not remove the saved audio. Try again.");
    } finally {
      setWorking(false);
      operation.current = false;
    }
  }

  return <Dialog open onOpenChange={open => { if (!open && !operation.current) onClose(); }}>
    <DialogContent className="sm:max-w-lg" onOpenAutoFocus={event => {
      event.preventDefault();
      dismissButton.current?.focus();
    }}>
      <DialogHeader>
        <DialogTitle>{results ? "Audio removal results" : "Remove saved audio"}</DialogTitle>
        <DialogDescription>Removal is permanent. Playback and transcription from this copy will be unavailable. Your transcript, summary, notes and references stay in the library.</DialogDescription>
      </DialogHeader>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      {!checks && !error && <p role="status">Checking saved audio…</p>}
      {checks && <ul className="max-h-52 divide-y divide-border overflow-y-auto rounded-md border border-border text-sm">
        {checks.map(check => {
          const outcome = results?.find(result => result.id === check.id);
          const detail = outcome
            ? outcome.removed_bytes === null ? outcome.reason : `Removed ${formatArchiveBytes(outcome.removed_bytes)}`
            : check.reason ?? formatArchiveBytes(check.bytes ?? 0);
          return <li key={check.id} className="px-3 py-2">
            <p className="break-words font-medium">{title.get(check.id)}</p>
            <p className="text-muted-foreground">{detail}</p>
          </li>;
        })}
      </ul>}
      {working && <p role="status" className="text-sm">Removing reviewed audio…</p>}
      <DialogFooter>
        <Button ref={dismissButton} variant={results ? "default" : "outline"} disabled={working} onClick={onClose}>{results || (checks && eligible.length === 0) ? "Done" : "Cancel"}</Button>
        {error && !checks && <Button variant="outline" onClick={() => setRevision(current => current + 1)}>Retry check</Button>}
        {!results && eligible.length > 0 && <Button variant="destructive" disabled={working} onClick={() => void remove()}>
          Remove {eligible.length === 1 ? "audio" : `${eligible.length} audio files`} ({formatArchiveBytes(bytes)})
        </Button>}
      </DialogFooter>
    </DialogContent>
  </Dialog>;
}
