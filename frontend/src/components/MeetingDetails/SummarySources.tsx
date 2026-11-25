'use client';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Accordion, AccordionContent, AccordionItem, AccordionTrigger } from '@/components/ui/accordion';
import { bookmarkTime } from '@/lib/library';
import { summarySourcePlaybackOffset } from '@/lib/summary-source-navigation';
import type { PaginatedTranscriptsResponse } from '@/types';

export interface ResolvedSummarySource {
  transcript_id: string; transcript_index: number; text: string; timestamp: number | null; stale: boolean;
}
interface Source { key: string; timestamp: number | null }
type SourcePreview = { snippet: string; stale: boolean };

export function SummarySources({ meetingId, revision, onReveal, onPlay, onNavigate, children }: {
  meetingId: string; revision: unknown; children: ReactNode;
  onReveal?: (source: ResolvedSummarySource, current: () => boolean) => Promise<void>;
  onPlay?: (seconds: number, current: () => boolean) => Promise<void>;
  onNavigate?: () => void;
}) {
  const [sources, setSources] = useState<Source[]>([]);
  const [previews, setPreviews] = useState<Record<string, SourcePreview | null>>({});
  const [previewRequested, setPreviewRequested] = useState(false);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [source, setSource] = useState<ResolvedSummarySource | null>(null);
  const [playbackOffset, setPlaybackOffset] = useState<number | null>(null);
  const [error, setError] = useState('');
  const [loadError, setLoadError] = useState(false);
  const version = useRef(0);
  const sourceKey = useRef('');
  const sourceTrigger = useRef<HTMLElement | null>(null);
  const navigated = useRef(false);
  useEffect(() => {
    let active = true;
    setOpen(false); setBusy(false); setSource(null); setSources([]); setLoadError(false);
    void invoke<Source[]>('api_get_summary_sources', { meetingId }).then(value => {
      if (active) setSources(value);
    }).catch(() => { if (active) setLoadError(true); });
    return () => { active = false; version.current++; };
  }, [meetingId, revision]);
  useEffect(() => {
    setPreviews({});
    if (!previewRequested || !sources.length) return;
    let active = true;
    async function loadPreviews() {
      // Resolve through the saved source map in small batches when first opened.
      for (let index = 0; active && index < sources.length; index += 4) {
        const entries = await Promise.all(sources.slice(index, index + 4).map(async ({ key }) => {
          try {
            const value = await invoke<ResolvedSummarySource>('api_resolve_summary_source', { meetingId, key });
            const text = Array.from(value.text.trim().replace(/\s+/g, ' '));
            const snippet = text.length > 160 ? `${text.slice(0, 159).join('')}…` : text.join('');
            return [key, { snippet, stale: value.stale }] as const;
          } catch {
            return [key, null] as const;
          }
        }));
        if (active) setPreviews(previous => ({ ...previous, ...Object.fromEntries(entries) }));
      }
    }
    void loadPreviews();
    return () => { active = false; };
  }, [meetingId, sources, previewRequested]);

  const getPlaybackOffset = (value: ResolvedSummarySource) => summarySourcePlaybackOffset(value, async index => {
    const page = await invoke<PaginatedTranscriptsResponse>('api_get_meeting_transcripts', { meetingId, offset: index, limit: 1 });
    return page.transcripts[0];
  });

  async function inspect(key: string, trigger: HTMLElement) {
    const request = ++version.current;
    sourceKey.current = key;
    sourceTrigger.current = trigger;
    navigated.current = false;
    setOpen(true); setBusy(true); setError(''); setSource(null); setPlaybackOffset(null);
    try {
      const value = await invoke<ResolvedSummarySource>('api_resolve_summary_source', { meetingId, key });
      const offset = await getPlaybackOffset(value);
      if (version.current === request) {
        setSource(value);
        setPlaybackOffset(offset);
      }
    } catch (e) { if (version.current === request) setError(typeof e === 'string' ? e : 'Could not open this source. Try again.'); }
    finally { if (version.current === request) setBusy(false); }
  }
  async function action(play: boolean) {
    if (!source || busy || !onReveal || (play && (!onPlay || playbackOffset === null))) return;
    const request = ++version.current;
    const current = () => version.current === request;
    setBusy(true); setError('');
    try {
      const target = await invoke<ResolvedSummarySource>('api_resolve_summary_source', { meetingId, key: sourceKey.current });
      if (!current()) return;
      const offset = play ? await getPlaybackOffset(target) : null;
      if (!current()) return;
      if (play && offset === null) {
        setSource(target);
        setPlaybackOffset(null);
        setError('Could not resolve the playback time for this passage.');
        return;
      }
      await onReveal(target, current);
      if (!current()) return;
      if (offset !== null) await onPlay?.(offset, current);
      if (!current()) return;
      navigated.current = true;
      setOpen(false);
    } catch { if (current()) setError('Could not open the passage. Try again after reloading the meeting.'); }
    finally { if (current()) setBusy(false); }
  }
  return <div onClickCapture={event => {
    const anchor = (event.target as Element).closest?.<HTMLElement>('a');
    const href = anchor?.getAttribute('href') ?? '';
    const marker = '#clawscribe-source-';
    // Accept only local hashes: a remote URL with this fragment is still external.
    if (anchor && href.startsWith(marker)) {
      event.preventDefault(); event.stopPropagation(); void inspect(href.slice(marker.length), anchor);
    }
  }}>
    {children}
    <div className="border-t border-border px-4 py-3 text-sm">
      {loadError ? <p role="status" className="text-muted-foreground">Source references could not be loaded. Reopen the meeting to retry.</p>
        : sources.length ? <Accordion type="single" collapsible onValueChange={value => { if (value) setPreviewRequested(true); }}><AccordionItem value="sources" className="group/details border-0"><AccordionTrigger className="cursor-pointer rounded py-0 font-medium hover:no-underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">Source passages ({sources.length})</AccordionTrigger><AccordionContent className="p-0">
          <p className="my-2 text-xs text-muted-foreground">Review these passages to check the summary’s claims. A source link does not guarantee the claim is correct.</p>
          <ul className="space-y-2">{sources.map((item, index) => {
            const preview = previews[item.key];
            return <li key={item.key}>
              <Button variant="outline" size="sm" className="h-auto min-h-8 w-full flex-wrap justify-start gap-x-2 gap-y-1 whitespace-normal py-1.5 text-left" onClick={event => void inspect(item.key, event.currentTarget)}>
                <span className="shrink-0 tabular-nums">{item.timestamp === null ? `Source ${index + 1}` : bookmarkTime(item.timestamp)}</span>
                {preview?.stale && <span className="text-xs text-warning-foreground">Changed</span>}
                <span className="min-w-0 flex-1 break-words font-normal text-muted-foreground">
                  {preview === undefined ? 'Loading preview…' : preview?.snippet || 'Preview unavailable'}
                </span>
              </Button>
            </li>;
          })}</ul>
        </AccordionContent></AccordionItem></Accordion> : <p className="text-muted-foreground">No source links in this summary. Generate new notes to request links to supporting passages.</p>}
    </div>
    <Dialog open={open} onOpenChange={value => { if (!busy) setOpen(value); }}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-xl" onCloseAutoFocus={event => {
        event.preventDefault();
        if (navigated.current && onNavigate) onNavigate();
        else sourceTrigger.current?.focus();
      }}>
        <DialogHeader><DialogTitle>Source passage</DialogTitle><DialogDescription>Compare the transcript with the summary and the original audio.</DialogDescription></DialogHeader>
        {busy && <p role="status">Opening passage…</p>}
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        {source && <>
          {source.stale && <p role="alert" className="text-sm text-warning-foreground">This passage has changed since the summary was generated. The current transcript appears below. Regenerate notes to refresh the reference.</p>}
          <blockquote className="whitespace-pre-wrap break-words border-l-2 border-primary pl-3 text-sm">{source.text}</blockquote>
          <div className="flex flex-wrap gap-2">
            <Button disabled={busy || !onReveal} onClick={() => void action(false)}>Show in transcript</Button>
            <Button variant="outline" disabled={busy || !onReveal || !onPlay || playbackOffset === null} onClick={() => void action(true)}>Play from here</Button>
          </div>
          {!onPlay && <p className="text-xs text-muted-foreground">Playback is available when this meeting’s saved audio can be loaded.</p>}
          {onPlay && playbackOffset === null && !error && <p className="text-xs text-muted-foreground">Playback time is unavailable for this passage.</p>}
        </>}
      </DialogContent>
    </Dialog>
  </div>;
}
