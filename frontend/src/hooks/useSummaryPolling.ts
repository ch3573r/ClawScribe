import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

const POLL_INTERVAL_MS = 5000;
const MAX_POLLS = 200;

export function useSummaryPolling() {
  const polls = useRef(new Map<string, NodeJS.Timeout>());
  const [activeSummaryPolls, setActiveSummaryPolls] = useState(new Map<string, NodeJS.Timeout>());

  const stopSummaryPolling = useCallback((meetingId: string) => {
    const timer = polls.current.get(meetingId);
    if (timer !== undefined) {
      clearInterval(timer);
      polls.current.delete(meetingId);
      setActiveSummaryPolls(new Map(polls.current));
    }
  }, []);

  const startSummaryPolling = useCallback((
    meetingId: string,
    _processId: string,
    onUpdate: (result: any) => void | Promise<void>,
  ) => {
    stopSummaryPolling(meetingId);
    let pollCount = 0;
    let inFlight = false;
    const timer = setInterval(async () => {
      const isCurrent = () => polls.current.get(meetingId) === timer;
      if (!isCurrent()) return;
      pollCount++;
      if (pollCount >= MAX_POLLS) {
        stopSummaryPolling(meetingId);
        await onUpdate({
          status: 'error',
          error: 'Summary status could not be confirmed after 16 minutes 40 seconds. Reopen the meeting to check for saved notes, or stop generation before retrying.',
        });
        return;
      }
      if (inFlight) return;
      inFlight = true;
      let result: any;
      try {
        result = await invoke('api_get_summary', { meetingId });
      } catch (error) {
        result = { status: 'error', error: typeof error === 'string' ? error : error instanceof Error ? error.message : 'Could not check summary status.' };
      } finally {
        inFlight = false;
      }
      // A response from a stopped/replaced poll must not update its successor.
      if (!isCurrent()) return;
      if (result.status === 'idle') {
        result = { status: 'error', error: 'Summary generation is no longer active. Please generate the summary again.' };
      } else if (result.status === 'completed' && !result.data) {
        result = { status: 'error', error: 'Summary generation completed without saved content. Please try again.' };
      }
      if (['completed', 'error', 'failed', 'cancelled'].includes(result.status)) {
        stopSummaryPolling(meetingId);
      }
      await onUpdate(result);
    }, POLL_INTERVAL_MS);
    polls.current.set(meetingId, timer);
    setActiveSummaryPolls(new Map(polls.current));
  }, [stopSummaryPolling]);

  useEffect(() => () => {
    polls.current.forEach(timer => clearInterval(timer));
    polls.current.clear();
  }, []);

  return { activeSummaryPolls, startSummaryPolling, stopSummaryPolling };
}
