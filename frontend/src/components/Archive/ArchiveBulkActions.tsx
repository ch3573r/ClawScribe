"use client";

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useSidebar } from "@/components/Sidebar/SidebarProvider";
import { useRecordingState } from "@/contexts/RecordingStateContext";
import { addTag } from "@/lib/library";
import { discardMeetingDraft } from "@/lib/meetingDrafts";
import { indexedDBService } from "@/services/indexedDBService";
import { openStorageDialog } from "@/lib/storage-dialog-controller";
import type { ArchiveMeeting } from "./ArchiveAudioRemoval";
import { exportArchiveMeeting, type LocalExportFormat } from "./archive-bulk-export";

export type BulkAction = "tag" | "delete" | "export";
const labels = { tag: "Add project tags", delete: "Delete meetings", export: "Export meetings" };
const description = {
  tag: "Add tags to each selected meeting. Existing tags stay in place. Separate tags with commas.",
  delete: "Permanently remove the selected meetings, transcripts, and notes from ClawScribe.",
  export: "Save a file with the saved notes and transcript for each meeting. Choose each path in the save dialog. Cancel a save dialog to stop the batch.",
};

export function ArchiveBulkActions({ action, meetings, onClose, onDeleted, onReturnFocus }: {
  action: BulkAction;
  meetings: readonly ArchiveMeeting[];
  onClose: () => void;
  onDeleted: (ids: string[]) => void;
  onReturnFocus: () => void;
}) {
  const { projectTags, setCurrentMeeting, setMeetings } = useSidebar();
  const recording = useRecordingState();
  const recordingBusy = recording.isRecording || recording.isStarting || recording.isStopping || recording.isSaving || recording.isProcessing;
  const [tag, setTag] = useState("");
  const [deleteFiles, setDeleteFiles] = useState(false);
  const [format, setFormat] = useState<LocalExportFormat>("markdown");
  const [progress, setProgress] = useState("");
  const [error, setError] = useState("");
  const [result, setResult] = useState<{ count: number; stopped: boolean; failures: { title: string; message: string }[]; cleanup: boolean; cacheFailure: boolean } | null>(null);
  const pending = useRef(false);
  const active = useRef(true);
  const cancel = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    active.current = true;
    return () => { active.current = false; };
  }, []);

  const run = async () => {
    if (pending.current || result || (action === "delete" && recordingBusy)) return;
    pending.current = true;
    setError("");
    setProgress("Starting…");
    const deleted: string[] = [];
    const summary = { count: 0, stopped: false, failures: [] as { title: string; message: string }[], cleanup: false, cacheFailure: false };
    try {
      if (action === "tag") {
        const tags = addTag([], tag, projectTags);
        if (!tags.length) throw new Error("Enter at least one tag.");
        await invoke("add_archive_project_tags", { ids: meetings.map(meeting => meeting.id), tags });
        summary.count = meetings.length;
      } else {
        for (const [index, meeting] of meetings.entries()) {
          if (!active.current) break;
          setProgress(`${index + 1} of ${meetings.length} · ${meeting.title}`);
          try {
            if (action === "export") {
              if (!await exportArchiveMeeting(meeting, format)) { summary.stopped = true; break; }
            } else {
              const outcome = await invoke<{ cleanup_pending: boolean }>("api_delete_meeting", { meetingId: meeting.id, deleteFiles });
              discardMeetingDraft(meeting.id);
              summary.cleanup ||= outcome.cleanup_pending;
              deleted.push(meeting.id);
              setMeetings(rows => rows.filter(row => row.id !== meeting.id));
              setCurrentMeeting(current => current?.id === meeting.id ? null : current);
              try { await indexedDBService.deleteMeeting(meeting.id); }
              catch { summary.cacheFailure = true; }
            }
            summary.count++;
          } catch (cause) {
            summary.failures.push({ title: meeting.title, message: typeof cause === "string" ? cause : cause instanceof Error ? cause.message : "Try again." });
          }
        }
      }
      if (deleted.length) onDeleted(deleted);
      setResult(summary);
    } catch (cause) {
      setError(typeof cause === "string" ? cause : cause instanceof Error ? cause.message : "Try again.");
    } finally {
      pending.current = false;
      setProgress("");
    }
  };

  return <Dialog open onOpenChange={open => { if (!open && !pending.current) { active.current = false; onClose(); } }}>
    <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-y-auto"
      onOpenAutoFocus={event => { if (action === "delete") { event.preventDefault(); cancel.current?.focus(); } }}
      onCloseAutoFocus={event => { event.preventDefault(); onReturnFocus(); }}>
      <DialogHeader>
        <DialogTitle>{labels[action]}</DialogTitle>
        <DialogDescription>{meetings.length} selected. {description[action]}</DialogDescription>
      </DialogHeader>
      <form className="space-y-4" onSubmit={event => { event.preventDefault(); void run(); }}>
        {!result && action === "tag" && <label className="block space-y-2 text-sm">
          <span>Project tags (up to 20 per meeting, 60 characters each)</span>
          <Input value={tag} maxLength={2440} onChange={event => setTag(event.target.value)} disabled={!!progress} />
        </label>}
        {!result && action === "delete" && <label className="flex items-start gap-2 text-sm">
          <Checkbox className="mt-0.5" checked={deleteFiles} disabled={!!progress}
            onCheckedChange={checked => setDeleteFiles(checked === true)} />
          <span>Also remove verified recording files. Leave this unchecked to keep the files on disk.</span>
        </label>}
        {!result && action === "export" && <div className="space-y-2 text-sm">
          <label htmlFor="archive-bulk-format">File format</label>
          <Select value={format} onValueChange={value => setFormat(value as LocalExportFormat)} disabled={!!progress}>
            <SelectTrigger id="archive-bulk-format"><SelectValue /></SelectTrigger>
            <SelectContent><SelectItem value="markdown">Markdown (.md)</SelectItem><SelectItem value="word">Word (.docx)</SelectItem></SelectContent>
          </Select>
        </div>}
        {progress && <p role="status" className="text-sm text-muted-foreground">{progress}</p>}
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        {action === "delete" && recordingBusy && !result && <p role="status" className="text-sm text-muted-foreground">Finish the recording before deleting meetings.</p>}
        {result && <div className="space-y-2 text-sm">
          <p role="status">{result.count} of {meetings.length} {action === "delete" ? "deleted" : action === "tag" ? "tagged" : "exported"}.{result.stopped ? " Export stopped." : ""}</p>
          {result.failures.length > 0 && <ul aria-label="Meetings that could not be processed" className="list-disc space-y-1 pl-5 text-destructive">
            {result.failures.map((failure, index) => <li key={index}>{failure.title || "Untitled meeting"}. {failure.message}</li>)}
          </ul>}
          {result.cleanup && <Button type="button" variant="outline" onClick={() => { onClose(); window.setTimeout(openStorageDialog, 0); }}>Review pending file cleanup</Button>}
          {result.cacheFailure && <p>Some cached recovery copies could not be removed.</p>}
        </div>}
        <DialogFooter>
          {result ? <Button type="button" onClick={onClose}>Done</Button> : <>
            <Button ref={cancel} type="button" variant="outline" disabled={!!progress} onClick={onClose}>Cancel</Button>
            <Button type="submit" variant={action === "delete" ? "destructive" : "default"}
              disabled={!!progress || (action === "tag" && !tag.trim()) || (action === "delete" && recordingBusy)}>{labels[action]}</Button>
          </>}
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>;
}
