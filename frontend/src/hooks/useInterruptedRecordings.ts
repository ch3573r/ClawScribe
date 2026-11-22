"use client";

import { useCallback, useEffect, useState } from "react";
import { fetchRecoveryCandidates } from "@/hooks/useTranscriptRecovery";

/**
 * How many interrupted recordings there are right now (browser data and the
 * core's scan of the recording folders), and whether a scan should be retried.
 * It does nothing while `enabled` is false, for example during a recording.
 */
export function useInterruptedRecordings(enabled: boolean) {
  const [count, setCount] = useState(0);
  const [keptCount, setKeptCount] = useState(0);
  const [loading, setLoading] = useState(false);
  const [scanFailed, setScanFailed] = useState(false);
  const [hasUnverifiedFolders, setHasUnverifiedFolders] = useState(false);
  const [waitingForAudio, setWaitingForAudio] = useState(false);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    if (!enabled) {
      setCount(0);
      setKeptCount(0);
      setLoading(false);
      setScanFailed(false);
      setHasUnverifiedFolders(false);
      setWaitingForAudio(false);
      return;
    }
    let active = true;
    setLoading(true);
    setCount(0);
    setKeptCount(0);
    fetchRecoveryCandidates()
      .then(({ candidates, keptCount: kept, scanFailed: failed, scan }) => {
        if (active) {
          setCount(candidates.length);
          setKeptCount(kept);
          setScanFailed(failed);
          setHasUnverifiedFolders(candidates.some(candidate => candidate.folderUnverified));
          setWaitingForAudio(scan.waitingForAudio === true);
        }
      })
      .catch(() => { if (active) setScanFailed(true); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [enabled, tick]);

  const refresh = useCallback(() => setTick(value => value + 1), []);
  return { count, keptCount, loading, scanFailed, hasUnverifiedFolders, waitingForAudio, refresh };
}
