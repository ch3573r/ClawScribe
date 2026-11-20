import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { isTerminalSummary, type SummaryJobSnapshot } from '@/lib/summary-job-status';

const POLL_INTERVAL_MS = 5000;

export function useSummaryPolling() {
  type Listener = (result: any) => void | Promise<void>;
  const polls = useRef(new Map<string, NodeJS.Timeout>());
  // Who hears a poll's results. The starter is one listener; a meeting page that
  // opens while the poll runs adds another (see attachSummaryPolling) instead of
  // replacing it, so the starter's own completion handling still happens.
  const listeners = useRef(new Map<string, Set<Listener>>());
  const [activeSummaryPolls, setActiveSummaryPolls] = useState(new Map<string, NodeJS.Timeout>());

  // Polls started with no meeting page behind them (the automatic summary after a
  // recording ended elsewhere). Leaving that meeting's page must not stop them.
  const background = useRef(new Set<string>());

  const stopSummaryPolling = useCallback((meetingId: string, options?: { keepBackground?: boolean }) => {
    if (options?.keepBackground && background.current.has(meetingId)) return;
    const timer = polls.current.get(meetingId);
    if (timer !== undefined) {
      clearInterval(timer);
      polls.current.delete(meetingId);
      listeners.current.delete(meetingId);
      background.current.delete(meetingId);
      setActiveSummaryPolls(new Map(polls.current));
    }
  }, []);

  const startSummaryPolling = useCallback((
    meetingId: string,
    _processId: string,
    onUpdate: Listener,
    options?: { background?: boolean },
  ) => {
    stopSummaryPolling(meetingId);
    if (options?.background) background.current.add(meetingId);
    const heard = new Set<Listener>([onUpdate]);
    listeners.current.set(meetingId, heard);
    // Listeners are read before a finished poll is stopped, which clears them.
    const notify = async (result: any, targets: Listener[]) => {
      for (const listener of targets) {
        try { await listener(result); } catch (error) { console.error('Summary update handler failed:', error); }
      }
    };
    let inFlight = false;
    const refresh = async () => {
      const isCurrent = () => polls.current.get(meetingId) === timer;
      if (!isCurrent()) return;
      if (inFlight) return;
      inFlight = true;
      let result: any;
      try {
        result = await invoke<SummaryJobSnapshot>('api_get_summary', { meetingId });
      } catch (error) {
        result = { status: 'unavailable', error: typeof error === 'string' ? error : error instanceof Error ? error.message : 'Could not check summary status.' };
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
      const targets = [...heard];
      if (isTerminalSummary(result.status)) {
        stopSummaryPolling(meetingId);
      }
      await notify(result, targets);
    };
    const timer = setInterval(refresh, POLL_INTERVAL_MS);
    polls.current.set(meetingId, timer);
    setActiveSummaryPolls(new Map(polls.current));
  }, [stopSummaryPolling]);

  /**
   * Hears the results of the poll for a meeting. If one is running (for example
   * the background poll of an automatic summary) the listener joins it and the
   * starter keeps hearing it too; otherwise a poll of its own is started. Call the
   * returned function to let go: a poll that nobody else hears then stops, a
   * background poll carries on and finishes on its own.
   */
  const attachSummaryPolling = useCallback((meetingId: string, onUpdate: Listener): (() => void) => {
    const running = polls.current.has(meetingId);
    if (running) listeners.current.get(meetingId)?.add(onUpdate);
    else startSummaryPolling(meetingId, '', onUpdate);
    const heard = listeners.current.get(meetingId);
    return () => {
      // Only the poll this listener joined: a newer poll for the meeting has its own set.
      if (!heard || listeners.current.get(meetingId) !== heard) return;
      heard.delete(onUpdate);
      if (heard.size === 0 && !background.current.has(meetingId)) stopSummaryPolling(meetingId);
    };
  }, [startSummaryPolling, stopSummaryPolling]);

  useEffect(() => () => {
    polls.current.forEach(timer => clearInterval(timer));
    polls.current.clear();
    listeners.current.clear();
    background.current.clear();
  }, []);

  return { activeSummaryPolls, startSummaryPolling, stopSummaryPolling, attachSummaryPolling };
}
