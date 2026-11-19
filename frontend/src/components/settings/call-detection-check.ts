"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { CALL_PROVIDERS, type CallProvider, type CallScan } from "@/lib/autoRecord";
import { getCallDetectionStatus } from "@/services/teamsDetectionService";
import type { StatusTone } from "@/components/ui/settings-layout";

/**
 * The manual call check shared by the Recording page and Diagnostics. It runs
 * the same scan that starts recordings, with the apps the user selected.
 */
export interface CallCheckState {
  scan: CallScan | null;
  checkedAt: Date | null;
  failed: boolean;
  checking: boolean;
}

const idle: CallCheckState = { scan: null, checkedAt: null, failed: false, checking: false };

export function describeCallCheck(state: CallCheckState): { tone: StatusTone; text: string } {
  if (state.checking) return { tone: "active", text: "Checking…" };
  if (state.failed) return { tone: "warning", text: "Could not check meeting windows" };
  const scan = state.scan;
  if (!scan) return { tone: "off", text: "Not checked yet" };
  if (!scan.supported) return { tone: "off", text: "Call detection is available on Windows" };
  if (!scan.complete) return { tone: "warning", text: "Could not check all meeting windows" };
  const names = [...new Set(scan.active.map(call => CALL_PROVIDERS[call.provider]))];
  return names.length ? { tone: "active", text: `Call active in ${names.join(", ")}` } : { tone: "neutral", text: "No call right now" };
}

export function useCallDetectionCheck(providers: CallProvider[]) {
  const [state, setState] = useState<CallCheckState>(idle);
  const revision = useRef(0);
  const key = providers.join(",");

  useEffect(() => () => { revision.current++; }, []);

  /** Forget the last result and ignore a check that is still running. */
  const reset = useCallback(() => {
    revision.current++;
    setState(idle);
  }, []);

  const check = useCallback(async () => {
    const version = ++revision.current;
    setState(previous => ({ ...previous, checking: true }));
    try {
      const scan = await getCallDetectionStatus(key ? (key.split(",") as CallProvider[]) : []);
      if (version === revision.current) setState({ scan, checkedAt: new Date(), failed: false, checking: false });
    } catch {
      if (version === revision.current) setState({ scan: null, checkedAt: new Date(), failed: true, checking: false });
    }
  }, [key]);

  return { state, check, reset };
}

export const clockTime = (date: Date) => date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
