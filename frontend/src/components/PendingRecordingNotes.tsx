"use client";

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger,
} from "@/components/ui/dialog";

interface PendingNotes {
  recording_id: string;
  folder_name: string | null;
  first_note: string | null;
  note_count: number;
  include_in_summary: boolean;
}

interface PendingNotesPage {
  entries: PendingNotes[];
  has_more: boolean;
}

function entryName(entry: PendingNotes) {
  return entry.folder_name || (entry.note_count ? "Recording with no folder name" : "Summary preference only");
}

export function PendingRecordingNotes({ disabled }: { disabled: boolean }) {
  const [open, setOpen] = useState(false);
  const [page, setPage] = useState(0);
  const [revision, setRevision] = useState(0);
  const [result, setResult] = useState<PendingNotesPage | null>(null);
  const [selection, setSelection] = useState<PendingNotes | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const saving = useRef(false);
  const title = useRef<HTMLSpanElement>(null);

  useEffect(() => {
    if (!open) return;
    let active = true;
    setResult(null);
    setError("");
    void invoke<PendingNotesPage>("list_pending_recording_notes", { page }).then(data => {
      if (active) setResult(data);
    }).catch(() => {
      if (active) setError("Could not list pending notes.");
    });
    return () => { active = false; };
  }, [open, page, revision]);

  const confirm = async () => {
    if (!selection || disabled || saving.current) return;
    saving.current = true;
    setBusy(true);
    setError("");
    try {
      await invoke("delete_pending_recording_notes", {
        recordingId: selection.recording_id,
        noteCount: selection.note_count,
        includeInSummary: selection.include_in_summary,
      });
      setSelection(null);
      setRevision(value => value + 1);
      title.current?.focus();
    } catch (failure) {
      setError(typeof failure === "string" ? failure : "Could not update pending notes. Try again.");
    } finally {
      saving.current = false;
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={next => {
      if (saving.current) return;
      setOpen(next);
      if (next) {
        setPage(0);
        setResult(null);
        setError("");
        setSelection(null);
      }
    }}>
      <DialogTrigger asChild>
        <Button variant="outline" size="sm" disabled={disabled}>Manage pending notes</Button>
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle><span ref={title} tabIndex={-1}>{selection ? "Delete pending notes?" : "Pending recording notes"}</span></DialogTitle>
          <DialogDescription>
            {selection
              ? "This permanently removes the selected recording’s notes and summary preference. They will be gone even if you recover the recording later. Its recording files stay as they are."
              : "These notes await recovery. Keep them if their recording folder might have moved or become temporarily unavailable."}
          </DialogDescription>
        </DialogHeader>
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        {selection ? (
          <>
            <p className="break-words text-sm font-medium" title={selection.recording_id}>{entryName(selection)}</p>
            {selection.first_note && <p className="line-clamp-2 break-words text-sm text-muted-foreground">{selection.first_note}</p>}
            <DialogFooter>
              <Button autoFocus variant="outline" disabled={busy} onClick={() => { setSelection(null); setError(""); title.current?.focus(); }}>Cancel</Button>
              <Button variant="destructive" disabled={disabled || busy} onClick={confirm}>
                {busy ? "Deleting…" : "Delete notes and preference"}
              </Button>
            </DialogFooter>
          </>
        ) : result ? (
          <>
            {result.entries.length ? (
              <ul className="max-h-72 divide-y divide-border overflow-y-auto">
                {result.entries.map(entry => (
                  <li key={entry.recording_id} className="flex items-start justify-between gap-3 py-3 text-sm">
                    <div className="min-w-0">
                      <p className="break-words font-medium" title={entry.recording_id}>{entryName(entry)}</p>
                      <p className="text-muted-foreground">{entry.note_count} {entry.note_count === 1 ? "note" : "notes"}{entry.include_in_summary ? " · Summary priorities enabled" : ""}</p>
                      {entry.first_note && <p className="line-clamp-2 break-words">{entry.first_note}</p>}
                    </div>
                    <Button variant="outline" size="sm" disabled={disabled} onClick={() => setSelection(entry)}>Review removal</Button>
                  </li>
                ))}
              </ul>
            ) : <p className="text-sm text-muted-foreground">{page === 0 ? "No pending recording notes." : "No more pending recording notes."}</p>}
            {(page > 0 || result.has_more) && <DialogFooter>
              <Button variant="outline" size="sm" disabled={page === 0} onClick={() => setPage(value => value - 1)}>Previous</Button>
              <Button variant="outline" size="sm" disabled={!result.has_more} onClick={() => setPage(value => value + 1)}>Next</Button>
            </DialogFooter>}
          </>
        ) : error ? (
          <Button variant="outline" className="justify-self-start" onClick={() => setRevision(value => value + 1)}>Retry</Button>
        ) : (
          <p role="status" className="text-sm text-muted-foreground">Loading pending notes…</p>
        )}
      </DialogContent>
    </Dialog>
  );
}
