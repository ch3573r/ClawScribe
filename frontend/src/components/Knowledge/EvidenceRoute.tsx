'use client';

import { useEffect, useRef, useState } from 'react';
import { Button } from '@/components/ui/button';
import { evidenceIntent, finishEvidenceIntent, navigateEvidence } from '@/lib/knowledge-navigation';
import { knowledgeService } from '@/services/knowledgeService';

export function EvidenceRoute({ token, meetingId, audioReady, audioStatus, audioError, onReveal, onPlay }: {
  token: string | null | undefined;
  meetingId: string;
  audioReady: boolean;
  audioStatus: 'loading' | 'found' | 'missing';
  audioError: string | null;
  onReveal: (id: string, index: number, current: () => boolean) => Promise<void>;
  onPlay: (seconds: number, current: () => boolean) => Promise<void>;
}) {
  const completed = useRef('');
  const revealed = useRef('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [retry, setRetry] = useState(0);
  const [waitExpired, setWaitExpired] = useState(false);

  useEffect(() => {
    setWaitExpired(false);
    if (!token) return;
    const timer = setTimeout(() => setWaitExpired(true), 10000);
    return () => clearTimeout(timer);
  }, [token, retry]);

  useEffect(() => {
    if (!token || completed.current === token) return;
    const intent = evidenceIntent(token, meetingId);
    if (!intent) {
      setBusy(false);
      setError('This source link is no longer available. Open the source again.');
      return;
    }

    const waitingForAudio = intent.play && !audioReady && !audioError && audioStatus !== 'missing' && !waitExpired;
    if (waitingForAudio && revealed.current === token) {
      setError('');
      setBusy(true);
      return;
    }
    let active = true;
    const current = () => active;
    setError('');
    setBusy(true);

    // Reveal the text immediately. Recheck the evidence once the player is ready.
    void navigateEvidence(intent.reference, intent.play && !waitingForAudio, {
      current,
      resolve: knowledgeService.resolve,
      reveal: async (id, index, stillCurrent) => {
        if (revealed.current === token) return;
        await onReveal(id, index, stillCurrent);
        if (stillCurrent()) revealed.current = token;
      },
      play: onPlay,
    }, intent.contextReference).then(result => {
      if (!current() || !result || waitingForAudio) return;
      completed.current = token;
      finishEvidenceIntent(token);
      setBusy(false);
    }).catch(failure => {
      if (!current()) return;
      setBusy(false);
      setError(failure instanceof Error ? failure.message : 'Could not open the source. Retry.');
    });
    return () => { active = false; };
  }, [token, meetingId, audioReady, audioStatus, audioError, onReveal, onPlay, retry, waitExpired]);

  if (!token || (!busy && !error)) return null;
  const intent = evidenceIntent(token, meetingId);
  const waitingForAudio = revealed.current === token && intent?.play && !audioReady;
  return <div className="flex flex-wrap items-center gap-3 border-b border-border bg-card px-4 py-3 text-sm">
    {busy && <p role="status">{waitingForAudio ? 'Waiting for saved audio…' : 'Opening transcript passage…'}</p>}
    {error && <>
      <p role="alert" className="text-destructive">{error}</p>
      {intent && <Button variant="outline" size="sm" onClick={() => {
        revealed.current = '';
        setRetry(value => value + 1);
      }}>Retry opening passage</Button>}
    </>}
  </div>;
}
