"use client";

import { useCallback, useEffect, useState } from "react";
import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  microsoftExportService,
  type MicrosoftConnectionInfo,
  type NotebookInfo,
  type SectionInfo,
  type PlanInfo,
  type BucketInfo,
} from "@/services/microsoftExportService";

export function useMicrosoftExport() {
  const [connection, setConnection] = useState<MicrosoftConnectionInfo>({
    state: "not_connected",
    userDisplayName: null,
    userEmail: null,
  });
  const [signingIn, setSigningIn] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [notebooks, setNotebooks] = useState<NotebookInfo[]>([]);
  const [sections, setSections] = useState<SectionInfo[]>([]);
  const [plans, setPlans] = useState<PlanInfo[]>([]);
  const [buckets, setBuckets] = useState<BucketInfo[]>([]);

  const [loadingNotebooks, setLoadingNotebooks] = useState(false);
  const [loadingSections, setLoadingSections] = useState(false);
  const [loadingPlans, setLoadingPlans] = useState(false);
  const [loadingBuckets, setLoadingBuckets] = useState(false);

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
        setSections([]);
        setPlans([]);
        setBuckets([]);
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
      setSections([]);
      setPlans([]);
      setBuckets([]);
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
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoadingNotebooks(false);
    }
  }, []);

  const loadSections = useCallback(async (notebookId: string) => {
    setLoadingSections(true);
    try {
      setSections(await microsoftExportService.listSections(notebookId));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoadingSections(false);
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

  return {
    connection,
    signingIn,
    error,
    signIn,
    signOut,
    notebooks,
    sections,
    plans,
    buckets,
    loadingNotebooks,
    loadingSections,
    loadingPlans,
    loadingBuckets,
    loadNotebooks,
    loadSections,
    loadPlans,
    loadBuckets,
    createNotebook,
    createBucket,
    refreshStatus,
  };
}
