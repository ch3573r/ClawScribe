"use client";

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { toast } from "sonner";
import { usePlatform } from "@/hooks/usePlatform";
import type { UpcomingEntry } from "@/hooks/useUpcomingEvents";
import { calendarLinkFromEvent, microsoftCalendarRevision } from "@/lib/meetingCalendar";
import { confirmCallPrompt } from "@/services/callDetectionService";

interface JoinView {
  id: string;
  subject: string;
  opening: boolean;
  promptKey: string | null;
}

export function supportsJoinRecording({ event, sourceId, allDay }: UpcomingEntry): boolean {
  if (sourceId || allDay || event.isCancelled || !event.joinUrl || !event.start ||
      !(Date.parse(event.end ?? "") > Math.max(Date.now(), Date.parse(event.start)))) return false;
  try {
    const url = new URL(event.joinUrl);
    return url.protocol === "https:" && !url.username && !url.password && !url.port &&
      ["teams.microsoft.com", "teams.cloud.microsoft", "teams.live.com"].includes(url.hostname) &&
      /^\/(?:meet\/\d+|l\/meetup-join\/[a-zA-Z0-9_%:.@-]+\/0)\/*$/.test(url.pathname);
  } catch { return false; }
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
    if (!windows || pending || account !== microsoftCalendarRevision || !supportsJoinRecording(entry)) return;
    return action(() => invoke("join_and_record", {
      link: calendarLinkFromEvent(entry.event), end: entry.event.end, url: entry.event.joinUrl,
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
