'use client';

import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { errorText } from '@/lib/errorText';

type HandoffStatus = {
  meeting_id: string;
  state: 'not_sent' | 'skipped' | 'pending' | 'submitted' | 'failed' | 'unknown';
  message: string;
  destination: string | null;
  attempted_destination: string | null;
  includes_audio_path: boolean;
  review_token: string | null;
  can_submit: boolean;
  can_reconcile: boolean;
};

export function OpenClawHandoff({ meetingId }: { meetingId: string }) {
  const [status, setStatus] = useState<HandoffStatus | null>(null);
  const [review, setReview] = useState<HandoffStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const request = useRef(0);
  const action = useRef(false);

  const refresh = useCallback(async () => {
    const current = ++request.current;
    try {
      const value = await invoke<HandoffStatus>('get_openclaw_submission_status', { meetingId });
      if (current === request.current) setStatus(value);
    } catch (caught) {
      if (current === request.current) setError(errorText(caught, 'Could not read the OpenClaw handoff'));
    }
  }, [meetingId]);

  useEffect(() => {
    let mounted = true;
    const subscription = listen<{ meeting_id: string }>('openclaw-handoff-changed', event => {
      if (mounted && event.payload.meeting_id === meetingId) void refresh();
    });
    const repaired = listen<{ meeting_id: string }>('transcript-files-synchronized', event => {
      if (mounted && event.payload.meeting_id === meetingId) void refresh();
    });
    const settings = listen('openclaw-config-changed', () => { if (mounted) void refresh(); });
    const subscriptions = [subscription, repaired, settings];
    void Promise.allSettled(subscriptions).then(() => { if (mounted) void refresh(); });
    return () => {
      mounted = false;
      request.current += 1;
      for (const pending of subscriptions) void pending.then(unlisten => unlisten(), () => {});
    };
  }, [meetingId, refresh]);

  async function apply(command: string, args: Record<string, unknown>) {
    if (action.current) return;
    action.current = true;
    setBusy(true);
    setError(null);
    try {
      await invoke<HandoffStatus>(command, { meetingId, ...args });
      setReview(null);
    } catch (caught) {
      setError(errorText(caught, 'Could not update the OpenClaw handoff'));
    } finally {
      action.current = false;
      setBusy(false);
      await refresh();
    }
  }

  if ((!status || (!status.can_submit && status.state === 'not_sent')) && !error) return null;

  const isReconciliation = review?.can_reconcile === true;
  const isRetry = review?.state === 'failed';
  return <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-b border-border px-4 py-2 text-xs">
    <div role="status" className="min-w-0 flex-1 break-words text-muted-foreground">
      <span className="font-medium text-foreground">OpenClaw handoff</span>
      {status?.message && <span> · {status.message}</span>}
      {error && !review && <span className="text-destructive"> · {error}</span>}
    </div>
    {status && (status.can_submit || status.can_reconcile) &&
      <Button size="sm" variant="ghost" disabled={busy} onClick={() => { setError(null); setReview(status); }}>
        {status.can_reconcile ? 'Review delivery' : status.state === 'failed' ? 'Review retry' : 'Review handoff'}
      </Button>}

    <Dialog open={review !== null} onOpenChange={open => { if (!open && !busy) setReview(null); }}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{isReconciliation ? 'Verify OpenClaw delivery' : isRetry ? 'Retry OpenClaw handoff' : 'Send meeting to OpenClaw'}</DialogTitle>
          <DialogDescription className="break-words">
            {isReconciliation
              ? `Check ${review?.attempted_destination ?? 'the original OpenClaw destination'} for this meeting. Record whether it received the handoff after processing has settled.`
              : `Send the saved meeting details, transcript and recording folder path to ${review?.destination ?? 'the configured OpenClaw endpoint'}.${review?.includes_audio_path ? ' This also includes the local audio file path.' : ''}`}
          </DialogDescription>
        </DialogHeader>
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        <DialogFooter>
          <Button variant="ghost" disabled={busy} onClick={() => setReview(null)}>Close</Button>
          {isReconciliation ? <>
            {review?.attempted_destination && <Button variant="outline" disabled={busy}
              onClick={() => void apply('reconcile_openclaw_handoff', { received: false })}>Confirmed absent</Button>}
            <Button disabled={busy} onClick={() => void apply('reconcile_openclaw_handoff', { received: true })}>Confirmed received</Button>
          </> : <Button disabled={busy || !review?.review_token} onClick={() => void apply('submit_meeting_folder_to_openclaw', { reviewToken: review?.review_token })}>
            {busy ? 'Sending…' : isRetry ? 'Retry handoff' : 'Send transcript'}
          </Button>}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>;
}
