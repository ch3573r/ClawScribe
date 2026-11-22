"use client";

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { toast } from "sonner";
import { usePlatform } from "@/hooks/usePlatform";
import type { UpcomingEntry } from "@/hooks/useUpcomingEvents";
import { calendarLinkFromEvent, microsoftCalendarRevision } from "@/lib/meetingCalendar";
import { confirmCallPrompt } from "@/services/callDetectionService";
import { teamsJoinLink } from "@/lib/teams-join-link";

interface JoinView {
  id: string;
  subject: string;
  opening: boolean;
  promptKey: string | null;
}

export function supportsJoinRecording({ event, allDay }: UpcomingEntry): boolean {
  if (allDay || event.isCancelled || !event.joinUrl || !event.start ||
      !(Date.parse(event.end ?? "") > Math.max(Date.now(), Date.parse(event.start)))) return false;
  return teamsJoinLink(event.joinUrl) !== null;
}

export function useJoinRecording() {
  const windows = usePlatform() === "windows";
  const [pending, setPending] = useState<JoinView | null>(null);
  const [busy, setBusy] = useState(false);
  const working = useRef(false);
  const account = microsoftCalendarRevision;

  useEffect(() => {
    if (!windows) return;
    let disposed = false;
    let version = 0;
    const refresh = async () => {
      const read = ++version;
      const view = await invoke<JoinView | null>("get_join_recording");
      if (!disposed && read === version) setPending(view);
    };
    const sync = () => void refresh().catch(() => {});
    const listener = listen("join-recording-changed", sync);
    void listener.then(sync).catch(() => {});
    return () => { disposed = true; void listener.then(remove => remove()).catch(() => {}); };
  }, [windows]);

  async function action(run: () => Promise<unknown>) {
    if (working.current) return;
    working.current = true;
    setBusy(true);
    try { await run(); }
    catch (error) {
      toast.error(typeof error === "string" ? error : "Could not update the recording request.");
    } finally {
      working.current = false;
      setBusy(false);
    }
  }

  function join(entry: UpcomingEntry) {
    if (!windows || pending || (!entry.sourceId && account !== microsoftCalendarRevision) || !supportsJoinRecording(entry)) return;
    return action(() => invoke("join_and_record", {
      link: calendarLinkFromEvent({ ...entry.event, sourceId: entry.sourceId }), end: entry.event.end, url: entry.event.joinUrl,
    }));
  }

  // Cancellation is available even while the baseline scan and launch command are outstanding.
  function cancel() {
    if (pending) void invoke("cancel_join_recording", { id: pending.id })
      .catch(() => toast.error("Could not cancel the recording request."));
  }

  function confirm() {
    const key = pending?.promptKey;
    if (key) return action(() => confirmCallPrompt(key));
  }
  return { windows, pending, busy, join, cancel, confirm };
}
