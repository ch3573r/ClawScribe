"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { PageSection } from "@/components/ui/page-section";
import { ConfirmationModal } from "@/components/ConfirmationModel/confirmation-modal";
import { openStorageDialog } from "@/lib/storage-dialog-controller";

interface TrashedMeeting {
  id: string;
  title: string;
  deleted_at: string;
  expires_at: string;
  delete_recording_on_purge: boolean;
}

function localTime(timestamp: string) {
  return new Date(timestamp.replace(" ", "T") + "Z").toLocaleString(undefined, {
    dateStyle: "medium", timeStyle: "short",
  });
}

export function MeetingTrash() {
  const [meetings, setMeetings] = useState<TrashedMeeting[] | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [target, setTarget] = useState<TrashedMeeting | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const pending = useRef(false);
  const request = useRef(0);
  const refresh = useCallback(async () => {
    const version = ++request.current;
    setLoading(true);
    setError("");
    try {
      const rows = await invoke<TrashedMeeting[]>("list_trashed_meetings");
      if (version === request.current) setMeetings(rows);
    } catch {
      if (version === request.current) setError("Could not load Trash.");
    } finally {
      if (version === request.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    let active = true;
    const subscriptions = ["library-changed", "library-meeting-trashed", "library-meeting-deleted"].map(event =>
      listen(event, () => { if (active) void refresh(); }),
    );
    void refresh();
    subscriptions.forEach(subscription => void subscription.catch(() => {}));
    return () => {
      active = false;
      request.current++;
      subscriptions.forEach(subscription => void subscription.then(off => off()).catch(() => {}));
    };
  }, [refresh]);

  const change = async (meeting: TrashedMeeting, purge: boolean) => {
    if (pending.current) return;
    pending.current = true;
    setBusy(true);
    try {
      if (purge) {
        const cleanupPending = await invoke<boolean>("purge_trashed_meeting", { meetingId: meeting.id });
        toast.success("Meeting deleted permanently");
        if (cleanupPending) {
          toast.warning("Some files need attention", {
            action: { label: "Review files", onClick: openStorageDialog },
          });
        }
      } else {
        await invoke("restore_trashed_meeting", { meetingId: meeting.id });
        toast.success("Meeting restored");
      }
      setTarget(null);
      if (!purge) heading.current?.focus();
      setMeetings(previous => previous?.filter(row => row.id !== meeting.id) ?? null);
      void refresh();
    } catch (cause) {
      toast.error(purge ? "Could not delete meeting" : "Could not restore meeting", {
        description: typeof cause === "string" ? cause : "Try again.",
      });
      void refresh();
    } finally {
      pending.current = false;
      setBusy(false);
    }
  };

  return <PageSection className="rounded-lg border border-border bg-card px-5 py-4">
    <h2 ref={heading} tabIndex={-1} className="text-base font-semibold">Trash</h2>
    <p className="mt-1 text-sm text-muted-foreground">Restore meetings within 30 days. After that, ClawScribe permanently deletes them.</p>
    {loading && meetings === null && <p role="status" className="mt-4 text-sm text-muted-foreground">Loading Trash…</p>}
    {error && <div role="alert" className="mt-4 flex items-center gap-3 text-sm text-destructive">
      {error}<Button variant="outline" size="sm" disabled={loading} onClick={() => void refresh()}>Retry</Button>
    </div>}
    {meetings?.length === 0 && !error && <p className="mt-4 text-sm text-muted-foreground">Trash is empty.</p>}
    {!!meetings?.length && <ul className="mt-3 divide-y divide-border">
      {meetings.map(meeting => <li key={meeting.id} className="flex flex-wrap items-center justify-between gap-3 py-3">
        <div className="min-w-0">
          <p className="break-words text-sm font-medium">{meeting.title || "Untitled meeting"}</p>
          <p className="mt-1 text-xs text-muted-foreground">Moved {localTime(meeting.deleted_at)} · Expires {localTime(meeting.expires_at)}</p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button variant="ghost" size="sm" disabled={busy} onClick={() => setTarget(meeting)}>Delete permanently</Button>
          <Button variant="outline" size="sm" disabled={busy} onClick={() => void change(meeting, false)}>Restore</Button>
        </div>
      </li>)}
    </ul>}
    <ConfirmationModal
      isOpen={!!target}
      title={`Permanently delete “${target?.title || "Untitled meeting"}”?`}
      text={target?.delete_recording_on_purge
        ? "Delete the meeting, its transcript, notes, and verified recording files. This action cannot be undone."
        : "Delete the meeting, its transcript, and notes. Any recording files will stay on disk. This action cannot be undone."}
      confirmLabel={busy ? "Deleting…" : "Delete permanently"}
      confirmDisabled={busy}
      returnFocus={heading}
      onConfirm={() => { if (target) void change(target, true); }}
      onCancel={() => { if (!pending.current) setTarget(null); }}
    />
  </PageSection>;
}
