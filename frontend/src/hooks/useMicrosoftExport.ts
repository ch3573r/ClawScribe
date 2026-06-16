"use client";

import { useCallback, useEffect, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
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
  const [userCode, setUserCode] = useState<string | null>(null);
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
    listen<{ state: string; userDisplayName?: string; userEmail?: string }>(
      "microsoft-auth-complete",
      (event) => {
        setSigningIn(false);
        setUserCode(null);
        setConnection({
          state: event.payload.state as MicrosoftConnectionInfo["state"],
          userDisplayName: event.payload.userDisplayName ?? null,
          userEmail: event.payload.userEmail ?? null,
        });
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
      const resp = await microsoftExportService.signIn();
      setUserCode(resp.userCode);
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

  return {
    connection,
    signingIn,
    userCode,
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
    refreshStatus,
  };
}
