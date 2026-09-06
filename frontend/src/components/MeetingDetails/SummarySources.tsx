'use client';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';

export interface ResolvedSummarySource {
  transcript_id: string; transcript_index: number; text: string; timestamp: number | null; stale: boolean;
}
interface Source { key: string; timestamp: number | null }
export function SummarySources({ meetingId, revision, onReveal, onPlay, children }: {
  meetingId: string; revision: unknown; children: ReactNode;
  onReveal?: (source: ResolvedSummarySource) => Promise<void>;
  onPlay?: (seconds: number) => Promise<void>;
}) {
  const [sources, setSources] = useState<Source[]>([]);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [source, setSource] = useState<ResolvedSummarySource | null>(null);
  const [error, setError] = useState('');
  const [loadError, setLoadError] = useState(false);
  const version = useRef(0);
  useEffect(() => {
    const request = ++version.current;
    setOpen(false); setSource(null); setSources([]); setLoadError(false);
    void invoke<Source[]>('api_get_summary_sources', { meetingId }).then(value => {
      if (version.current === request) setSources(value);
    }).catch(() => { if (version.current === request) setLoadError(true); });
    return () => { version.current++; };
  }, [meetingId, revision]);
  async function inspect(key: string) {
    const request = ++version.current;
    setOpen(true); setBusy(true); setError(''); setSource(null);
    try {
      const value = await invoke<ResolvedSummarySource>('api_resolve_summary_source', { meetingId, key });
      if (version.current === request) setSource(value);
    } catch (e) { if (version.current === request) setError(typeof e === 'string' ? e : 'Could not open this source. Try again.'); }
    finally { if (version.current === request) setBusy(false); }
  }
  async function action(play: boolean) {
    if (!source || source.stale) return;
    setBusy(true); setError('');
    try {
      if (play && source.timestamp !== null) await onPlay?.(source.timestamp);
      else await onReveal?.(source);
      setOpen(false);
    } catch { setError('Could not open the passage. Try again after reloading the meeting.'); }
    finally { setBusy(false); }
  }
  return <div onClickCapture={event => {
    const anchor = (event.target as Element).closest?.('a');
    const href = anchor?.getAttribute('href') ?? '';
    const marker = '#clawscribe-source-';
    // Accept only local hashes: a remote URL with this fragment is still external.
    if (href.startsWith(marker)) {
      event.preventDefault(); event.stopPropagation(); void inspect(href.slice(marker.length));
    }
  }}>
    {children}
    <div className="border-t border-border px-4 py-3 text-sm">
      {loadError ? <p role="status" className="text-muted-foreground">Source references could not be loaded. Reopen the meeting to retry.</p>
        : sources.length ? <details><summary className="cursor-pointer rounded font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">Source passages ({sources.length})</summary>
          <p className="my-2 text-xs text-muted-foreground">Review these passages to check the summary’s claims. A source link does not guarantee the claim is correct.</p>
          <div className="flex flex-wrap gap-2">{sources.map((item, index) => <Button key={item.key} variant="outline" size="sm" onClick={() => void inspect(item.key)}>
            {item.timestamp === null ? `Source ${index + 1}` : new Date(item.timestamp * 1000).toISOString().slice(11, 19)}
          </Button>)}</div>
        </details> : <p className="text-muted-foreground">No source links in this summary. Generate new notes to request links to supporting passages.</p>}
    </div>
    <Dialog open={open} onOpenChange={value => { if (!busy) setOpen(value); }}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-xl">
        <DialogHeader><DialogTitle>Source passage</DialogTitle><DialogDescription>Compare the transcript with the summary and the original audio.</DialogDescription></DialogHeader>
        {busy && <p role="status">Opening passage…</p>}
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        {source && <>
          {source.stale && <p role="alert" className="text-sm text-amber-700 dark:text-amber-400">This passage has changed since the summary was generated. The current transcript appears below. Regenerate notes to refresh the reference.</p>}
          <blockquote className="whitespace-pre-wrap break-words border-l-2 border-primary pl-3 text-sm">{source.text}</blockquote>
          <div className="flex flex-wrap gap-2">
            <Button disabled={busy || source.stale || !onReveal} onClick={() => void action(false)}>Show in transcript</Button>
            <Button variant="outline" disabled={busy || source.stale || !onPlay || source.timestamp === null} onClick={() => void action(true)}>Play from here</Button>
          </div>
          {!onPlay && <p className="text-xs text-muted-foreground">Playback is available when this meeting’s saved audio can be loaded.</p>}
        </>}
      </DialogContent>
    </Dialog>
  </div>;
}
