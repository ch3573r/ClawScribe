'use client';
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { bookmarkTime, MeetingBookmark } from '@/lib/library';

export function LiveBookmarkButton({ disabled }: { disabled?: boolean }) {
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  return <Button variant="outline" size="sm" disabled={disabled || busy} onClick={async () => {
    if (pending.current) return;
    pending.current = true; setBusy(true);
    try { await invoke('add_meeting_bookmark', { meetingId: null, seconds: null, label: 'Review this' }); toast.success('Meeting bookmarked', { description: 'Rename it in the saved meeting.' }); }
    catch (error) { toast.error('Could not add bookmark', { description: String(error) }); }
    finally { pending.current = false; setBusy(false); }
  }}>{busy ? 'Saving…' : 'Bookmark'}</Button>;
}

export function MeetingBookmarks({ meetingId, currentTime, onSeek }: { meetingId: string; currentTime?: number; onSeek?: (time: number) => void }) {
  const [bookmarks, setBookmarks] = useState<MeetingBookmark[]>([]);
  const [label, setLabel] = useState('Review this');
  const [seconds, setSeconds] = useState('0');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [revision, setRevision] = useState(0);
  const [editing, setEditing] = useState<string | null>(null);
  const [editLabel, setEditLabel] = useState('');
  const pending = useRef(false);
  useEffect(() => {
    let active = true; let request = 0;
    const load = async () => { const version = ++request; try { const rows = await invoke<MeetingBookmark[]>('list_meeting_bookmarks', { meetingId }); if (active && version === request) { setBookmarks(rows); setError(''); } } catch { if (active && version === request) setError('Could not load bookmarks.'); } };
    setBookmarks([]); void load();
    const subscription = listen('library-changed', () => { void load(); });
    void subscription.catch(() => { if (active) setError('Could not watch bookmark changes. Reopen this meeting.'); });
    return () => { active = false; void subscription.then(unlisten => unlisten()).catch(() => {}); };
  }, [meetingId, revision]);
  const change = async (command: string, args: Record<string, unknown>) => {
    if (pending.current) return;
    pending.current = true; setBusy(true);
    try { await invoke(command, args); setEditing(null); setRevision(r => r + 1); }
    catch (error) { toast.error('Could not update bookmark', { description: String(error) }); }
    finally { pending.current = false; setBusy(false); }
  };
  return <details className="border-b border-border px-3 py-2 text-sm">
    <summary className="cursor-pointer font-medium">Bookmarks ({bookmarks.length})</summary>
    {error && <p role="alert" className="mt-2 text-destructive">{error} <button className="underline" onClick={() => setRevision(r => r + 1)}>Retry</button></p>}
    <div className="mt-2 flex flex-wrap gap-2">
      <label className="grid min-w-0 flex-1 gap-1">Label<input className="w-full rounded border border-input bg-background p-1" maxLength={160} value={label} onChange={e => setLabel(e.target.value)} /></label>
      <label className="grid gap-1">Seconds<input className="w-24 rounded border border-input bg-background p-1" type="number" min={0} max={604800} step="0.1" value={seconds} onChange={e => setSeconds(e.target.value)} /></label>
      {currentTime !== undefined && <Button size="sm" variant="outline" onClick={() => setSeconds(currentTime.toFixed(1))}>Use playback time</Button>}
      <Button size="sm" disabled={busy || !label.trim() || !seconds.trim() || !Number.isFinite(Number(seconds)) || Number(seconds) < 0} onClick={() => change('add_meeting_bookmark', { meetingId, seconds: Number(seconds), label })}>Add bookmark</Button>
    </div>
    <ul className="mt-2 max-h-48 space-y-2 overflow-y-auto">{bookmarks.map(mark => <li key={mark.id} className="flex flex-wrap items-center gap-1">
      {editing === mark.id ? <>
        <input aria-label="Bookmark label" maxLength={160} className="min-w-0 flex-1 rounded border border-input bg-background p-1" value={editLabel} onChange={e => setEditLabel(e.target.value)} />
        <Button size="sm" disabled={busy || !editLabel.trim()} onClick={() => change('rename_meeting_bookmark', { id: mark.id, label: editLabel })}>Save</Button>
        <Button size="sm" variant="ghost" disabled={busy} onClick={() => setEditing(null)}>Cancel</Button>
      </> : <>
      <button className="w-full truncate text-left text-primary underline disabled:text-muted-foreground" disabled={!onSeek} onClick={() => onSeek?.(mark.seconds)} title={`${bookmarkTime(mark.seconds)} · ${mark.label}`}>{bookmarkTime(mark.seconds)} · {mark.label}</button>
      <Button size="sm" variant="ghost" disabled={busy} aria-label={`Rename bookmark ${mark.label}`} onClick={() => { setEditing(mark.id); setEditLabel(mark.label); }}>Rename</Button>
      <Button size="sm" variant="ghost" disabled={busy} aria-label={`Remove bookmark ${mark.label}`} onClick={() => change('delete_meeting_bookmark', { id: mark.id })}>Remove</Button>
      </>}
    </li>)}</ul>
    {!onSeek && <p className="mt-2 text-xs text-muted-foreground">Audio playback is unavailable for this meeting.</p>}
  </details>;
}
