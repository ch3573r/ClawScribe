'use client';
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Button } from '@/components/ui/button';
import { savedPassage, sourceLabel, type SavedHit } from '@/lib/archive-search';
import { bookmarkTime } from '@/lib/library';

type ResolvedHit = SavedHit & { note_context?: [string, string] };

export function SavedSearchRoute({ token, meetingId, onBookmark }: {
  token: string; meetingId: string; onBookmark: (id: string | null) => void;
}) {
  const [hit, setHit] = useState<ResolvedHit | null>(null);
  const [error, setError] = useState('');
  const [retry, setRetry] = useState(0);
  const preview = useRef<HTMLElement>(null);
  const noteMatch = useRef<HTMLElement>(null);
  useEffect(() => {
    const reference = savedPassage(token, meetingId);
    if (!reference) { setError('This search result expired. Run the search again.'); return; }
    let active = true;
    let generation = 0;
    const verify = async () => {
      const request = ++generation;
      setHit(null); setError(''); onBookmark(null);
      try {
        const current = await invoke<ResolvedHit | null>('resolve_saved_search_hit', { reference });
        if (!active || generation !== request) return;
        if (!current) { setError('This saved source changed or was removed. Run the search again.'); return; }
        setHit(current);
        if (current.reference.source === 'bookmark') onBookmark(current.reference.id);
      } catch {
        if (active && generation === request) setError('Could not check the saved source.');
      }
    };
    const subscriptions = ['library-changed', 'summary-job-changed'].map(event => listen(event, () => { if (active) void verify(); }));
    void Promise.allSettled(subscriptions).then(() => { if (active) void verify(); });
    return () => {
      active = false;
      onBookmark(null);
      subscriptions.forEach(sub => void sub.then(off => off()).catch(() => {}));
    };
  }, [meetingId, token, onBookmark, retry]);
  useEffect(() => {
    if (hit?.reference.source === 'summary') document.getElementById('meeting-summary-panel')?.focus();
    else if (hit?.reference.source === 'note') {
      preview.current?.focus();
      noteMatch.current?.scrollIntoView({ block: 'nearest' });
    }
  }, [hit]);
  return <section ref={preview} tabIndex={-1} aria-label="Search result" className="shrink-0 border-b border-border bg-card px-4 py-2 text-sm">
    {error ? <div role="alert">{error} {error === 'Could not check the saved source.' && <Button variant="ghost" size="sm" onClick={() => setRetry(value => value + 1)}>Retry</Button>}</div>
      : hit ? <><h2 className="font-medium">{sourceLabel[hit.reference.source]}{hit.seconds != null && ` · ${bookmarkTime(hit.seconds)}`}</h2>
        <blockquote className="max-h-32 overflow-y-auto whitespace-pre-wrap break-words">{hit.note_context
          ? <>{hit.note_context[0]}<mark ref={noteMatch} className="bg-accent text-accent-foreground">{hit.text}</mark>{hit.note_context[1]}</>
          : hit.text}</blockquote></>
        : <span role="status">Checking the saved source…</span>}
  </section>;
}
