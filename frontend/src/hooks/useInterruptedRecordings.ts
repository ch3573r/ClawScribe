"use client";

import { useCallback, useEffect, useState } from "react";
import { fetchRecoveryCandidates } from "@/hooks/useTranscriptRecovery";

/**
 * How many interrupted recordings there are right now (browser data and the
 * core's scan of the recording folders). Quiet: a failed read counts as none.
 * It does nothing while `enabled` is false, for example during a recording.
 */
export function useInterruptedRecordings(enabled: boolean): { count: number; loading: boolean; refresh: () => void } {
  const [count, setCount] = useState(0);
  const [loading, setLoading] = useState(false);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    if (!enabled) {
      setCount(0);
      return;
    }
    let active = true;
    setLoading(true);
    fetchRecoveryCandidates()
      .then(({ candidates }) => { if (active) setCount(candidates.length); })
      .catch(() => { if (active) setCount(0); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [enabled, tick]);

  const refresh = useCallback(() => setTick(value => value + 1), []);
  return { count, loading, refresh };
}
