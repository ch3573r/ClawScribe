"use client";

import { useCallback, useEffect, useState } from "react";
import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  isOneNoteLargeLibraryError,
  microsoftExportService,
  type MicrosoftConnectionInfo,
  type NotebookInfo,
  type PlanInfo,
  type BucketInfo,
  type ToDoListInfo,
  type CalendarEvent,
} from "@/services/microsoftExportService";
import { clearAllCalendarLinks } from "@/lib/meetingCalendar";

export function useMicrosoftExport() {
  const [connection, setConnection] = useState<MicrosoftConnectionInfo>({
    state: "not_connected",
    userDisplayName: null,
    userEmail: null,
  });
  const [signingIn, setSigningIn] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [notebooks, setNotebooks] = useState<NotebookInfo[]>([]);
  const [plans, setPlans] = useState<PlanInfo[]>([]);
  const [buckets, setBuckets] = useState<BucketInfo[]>([]);
  const [todoLists, setToDoLists] = useState<ToDoListInfo[]>([]);
  const [oneNoteNotebookListingLimited, setOneNoteNotebookListingLimited] = useState(false);

  const [loadingNotebooks, setLoadingNotebooks] = useState(false);
  const [loadingPlans, setLoadingPlans] = useState(false);
  const [loadingBuckets, setLoadingBuckets] = useState(false);
  const [loadingToDoLists, setLoadingToDoLists] = useState(false);

  const [calendarEvents, setCalendarEvents] = useState<CalendarEvent[]>([]);
  const [currentMeeting, setCurrentMeeting] = useState<CalendarEvent | null>(null);
  const [loadingCalendar, setLoadingCalendar] = useState(false);

  const refreshStatus = useCallback(async () => {
    try {
      const status = await microsoftExportService.connectionStatus();
      setConnection(status);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    void refreshStatus();
  }, [refreshStatus]);

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    listen<{ state: string; userDisplayName?: string; userEmail?: string; error?: string }>(
      "microsoft-auth-complete",
      (event) => {
        setSigningIn(false);
        if (event.payload.state !== "connected" && event.payload.error) {
          setError(event.payload.error);
        }
        setConnection({
          state: event.payload.state as MicrosoftConnectionInfo["state"],
          userDisplayName: event.payload.userDisplayName ?? null,
          userEmail: event.payload.userEmail ?? null,
        });
        // Any auth transition (sign-in, sign-out, account switch) invalidates
        // previously-loaded discovery data. Clear it everywhere so each panel
        // re-fetches fresh and never shows the prior account's notebooks/plans.
        setNotebooks([]);
        setPlans([]);
        setBuckets([]);
        setToDoLists([]);
        setOneNoteNotebookListingLimited(false);
        setCalendarEvents([]);
        setCurrentMeeting(null);
      },
    ).then((fn_) => {
      unlisten = fn_;
    });
    return () => {
      unlisten?.();
    };
  }, []);

  const signIn = useCallback(async () => {
    setSigningIn(true);
    setError(null);
    try {
      await microsoftExportService.signIn();
      setConnection((prev) => ({ ...prev, state: "connecting" }));
    } catch (e) {
      setSigningIn(false);
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  const signOut = useCallback(async () => {
    setError(null);
    try {
      await microsoftExportService.signOut();
      setConnection({
        state: "not_connected",
        userDisplayName: null,
        userEmail: null,
      });
      setNotebooks([]);
      setPlans([]);
      setBuckets([]);
      setToDoLists([]);
      setOneNoteNotebookListingLimited(false);
      setCalendarEvents([]);
      setCurrentMeeting(null);
      // Drop any stored calendar associations (attendee PII) on sign-out.
      clearAllCalendarLinks();
      // Broadcast so the OTHER hook instances (OneNote / Planner panels, which
      // each call this hook separately) also reset and don't show stale data.
      // Sign-in already broadcasts this event from the backend.
      await emit("microsoft-auth-complete", { state: "not_connected" });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  const loadNotebooks = useCallback(async () => {
    setLoadingNotebooks(true);
    try {
      setNotebooks(await microsoftExportService.listNotebooks());
      setOneNoteNotebookListingLimited(false);
      setError(null);
    } catch (e) {
      if (isOneNoteLargeLibraryError(e)) {
        setNotebooks([]);
        setOneNoteNotebookListingLimited(true);
        setError(null);
      } else {
        setError(e instanceof Error ? e.message : String(e));
      }
    } finally {
      setLoadingNotebooks(false);
    }
  }, []);

  const loadPlans = useCallback(async () => {
    setLoadingPlans(true);
    try {
      setPlans(await microsoftExportService.listPlans());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoadingPlans(false);
    }
  }, []);

  const loadBuckets = useCallback(async (planId: string) => {
    setLoadingBuckets(true);
    try {
      setBuckets(await microsoftExportService.listBuckets(planId));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoadingBuckets(false);
    }
  }, []);

  const loadToDoLists = useCallback(async () => {
    setLoadingToDoLists(true);
    try {
      setToDoLists(await microsoftExportService.listToDoLists());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoadingToDoLists(false);
    }
  }, []);

  // Current/next meeting + the next 24h of events (with invited attendees).
  const loadCalendar = useCallback(async () => {
    setLoadingCalendar(true);
    try {
      const now = new Date();
      const end = new Date(now.getTime() + 24 * 60 * 60 * 1000);
      const [events, current] = await Promise.all([
        microsoftExportService.listCalendarEvents(
          now.toISOString(),
          end.toISOString(),
        ),
        microsoftExportService.currentOrNextMeeting(),
      ]);
      setCalendarEvents(events);
      setCurrentMeeting(current);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoadingCalendar(false);
    }
  }, []);

  // Create a notebook, fold it into the list, and return it so the caller can
  // select it. Returns null on failure (error surfaced via `error`).
  const createNotebook = useCallback(
    async (displayName: string): Promise<NotebookInfo | null> => {
      setError(null);
      try {
        const nb = await microsoftExportService.createNotebook(displayName);
        setNotebooks((prev) =>
          prev.some((n) => n.id === nb.id) ? prev : [...prev, nb],
        );
        setOneNoteNotebookListingLimited(false);
        return nb;
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        return null;
      }
    },
    [],
  );

  const createBucket = useCallback(
    async (planId: string, name: string): Promise<BucketInfo | null> => {
      setError(null);
      try {
        const bucket = await microsoftExportService.createBucket(planId, name);
        setBuckets((prev) =>
          prev.some((b) => b.id === bucket.id) ? prev : [...prev, bucket],
        );
        return bucket;
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        return null;
      }
    },
    [],
  );

  const createToDoList = useCallback(
    async (displayName: string): Promise<ToDoListInfo | null> => {
      setError(null);
      try {
        const list = await microsoftExportService.createToDoList(displayName);
        setToDoLists((prev) =>
          prev.some((existing) => existing.id === list.id) ? prev : [...prev, list],
        );
        return list;
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        return null;
      }
    },
    [],
  );

  return {
    connection,
    signingIn,
    error,
    signIn,
    signOut,
    notebooks,
    plans,
    buckets,
    todoLists,
    loadingNotebooks,
    oneNoteNotebookListingLimited,
    loadingPlans,
    loadingBuckets,
    loadingToDoLists,
    loadNotebooks,
    loadPlans,
    loadBuckets,
    loadToDoLists,
    createNotebook,
    createBucket,
    createToDoList,
    refreshStatus,
    calendarEvents,
    currentMeeting,
    loadingCalendar,
    loadCalendar,
  };
}
