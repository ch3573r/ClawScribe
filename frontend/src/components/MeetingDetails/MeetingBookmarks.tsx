'use client';
import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { BookmarkPlus } from 'lucide-react';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { Accordion, AccordionContent, AccordionItem, AccordionTrigger } from '@/components/ui/accordion';
import { cn } from '@/lib/utils';
import { bookmarkTime, MeetingBookmark } from '@/lib/library';
import { parseBookmarkTime } from '@/lib/bookmark-time';

interface LiveBookmarkProps {
  sessionId: string | null;
  disabled?: boolean;
  className?: string;
}

const liveButtonClassName = "h-auto gap-0 rounded-none p-0 whitespace-normal text-[length:inherit] font-normal hover:bg-transparent hover:text-current";

function saveLiveBookmark(sessionId: string, label: string) {
  return invoke('add_meeting_bookmark', { meetingId: null, seconds: null, sessionId, label });
}

export function LiveBookmarkButton({ sessionId, disabled, className }: LiveBookmarkProps) {
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  return <Button variant="ghost" type="button" className={cn(liveButtonClassName, className)} aria-label="Bookmark this moment" disabled={!sessionId || disabled || busy} onClick={async () => {
    if (!sessionId || disabled || pending.current) return;
    pending.current = true; setBusy(true);
    try { await saveLiveBookmark(sessionId, 'Review this'); toast.success('Bookmark saved', { description: 'Rename it in the saved meeting.' }); }
    catch (error) { toast.error('Could not add bookmark', { description: String(error) }); }
    finally { pending.current = false; setBusy(false); }
  }}>{busy ? <div className="animate-spin rounded-full h-4 w-4 border-b-2 border-current" /> : <BookmarkPlus size={16} />}</Button>;
}

export function MeetingBookmarks({ meetingId, focusedBookmark, currentTime, initialTime, duration, onSeek, addOpen = false, onAddOpenChange, onBookmarksChange, onReturnFocus }: { focusedBookmark?: string | null; addOpen?: boolean; onAddOpenChange?: (open: boolean) => void; onBookmarksChange?: (bookmarks: MeetingBookmark[]) => void; meetingId: string; currentTime?: number; initialTime?: number; duration?: number; onSeek?: (time: number) => void; onReturnFocus?: () => void }) {
  const [bookmarks, setBookmarks] = useState<MeetingBookmark[]>([]);
  const [section, setSection] = useState('bookmarks');
  const hasBookmarks = bookmarks.length > 0;
  // Notes made during recording arrive through the same saved list. Open the
  // section on its first populated load and respect collapses during refreshes.
  useEffect(() => { if (hasBookmarks) setSection('bookmarks'); }, [hasBookmarks]);
  const hasFocusTarget = !!focusedBookmark && bookmarks.some(mark => mark.id === focusedBookmark);
  useEffect(() => {
    if (!hasFocusTarget || !focusedBookmark) return;
    setSection('bookmarks');
    const frame = requestAnimationFrame(() => document.getElementById(`bookmark-row-${focusedBookmark}`)?.focus());
    return () => cancelAnimationFrame(frame);
  }, [focusedBookmark, hasFocusTarget]);
  const [label, setLabel] = useState('');
  const [time, setTime] = useState('0');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [revision, setRevision] = useState(0);
  const [editing, setEditing] = useState<string | null>(null);
  const [editLabel, setEditLabel] = useState('');
  const pending = useRef(false);
  const report = useRef(onBookmarksChange);
  report.current = onBookmarksChange;
  const [usePriorities, setUsePriorities] = useState<boolean | null>(null);
  const [prioritiesError, setPrioritiesError] = useState(false);
  useEffect(() => { setBookmarks([]); report.current?.([]); setError(''); setEditing(null); }, [meetingId]);
  useEffect(() => {
    let active = true;
    setUsePriorities(null);
    setPrioritiesError(false);
    void invoke<boolean>('get_meeting_summary_bookmarks', { meetingId }).then(enabled => {
      if (active) setUsePriorities(enabled);
    }).catch(() => {
      if (active) setPrioritiesError(true);
    });
    return () => { active = false; };
  }, [meetingId, revision]);
  // A new note starts at the playback position and is ready to type into.
  const wasOpen = useRef(false);
  useEffect(() => {
    if (addOpen && !wasOpen.current) {
      setLabel('');
      setTime(initialTime !== undefined ? String(initialTime) : currentTime?.toFixed(1) ?? '0');
    }
    wasOpen.current = addOpen;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [addOpen]);
  const refreshWithoutEvents = useRef<(() => Promise<void>) | null>(null);
  useEffect(() => {
    let active = true; let request = 0;
    let subscriptionError = '';
    const load = async () => { const version = ++request; try { const rows = await invoke<MeetingBookmark[]>('list_meeting_bookmarks', { meetingId }); if (active && version === request) { setBookmarks(rows); report.current?.(rows); setError(subscriptionError); } } catch { if (active && version === request) setError('Could not load bookmarks.'); } };
    const subscription = listen('library-changed', () => { void load(); });
    // Subscribe before the initial read so a saved change cannot fall between them.
    void subscription.then(() => { if (active) void load(); }).catch(() => {
      if (!active) return;
      subscriptionError = 'Automatic bookmark refresh is unavailable. Retry to reconnect.';
      refreshWithoutEvents.current = load;
      setError(subscriptionError);
      void load();
    });
    return () => { active = false; refreshWithoutEvents.current = null; void subscription.then(unlisten => unlisten()).catch(() => {}); };
  }, [meetingId, revision]);
  const change = async (command: string, args: Record<string, unknown>) => {
    if (pending.current) return;
    pending.current = true; setBusy(true);
    try { await invoke(command, args); if (command === 'add_meeting_bookmark') onAddOpenChange?.(false); setEditing(null); await refreshWithoutEvents.current?.(); }
    catch (error) { toast.error('Could not update bookmark', { description: String(error) }); }
    finally { pending.current = false; setBusy(false); }
  };
  const setSummaryPriorities = async (enabled: boolean) => {
    if (usePriorities === null || pending.current) return;
    pending.current = true;
    setBusy(true);
    try {
      await invoke('set_meeting_summary_bookmarks', { meetingId, enabled });
      setUsePriorities(enabled);
    } catch {
      toast.error('Could not save this choice');
    } finally {
      pending.current = false;
      setBusy(false);
    }
  };
  const seconds = parseBookmarkTime(time, duration);
  const invalidTime = time.trim().length > 0 && seconds === null;
  const addForm = (
    <form className="mt-2 grid gap-3" onSubmit={event => {
      event.preventDefault();
      if (!busy && seconds !== null) void change('add_meeting_bookmark', { meetingId, seconds, label: label.trim() || 'Review this' });
    }}>
      <label className="grid gap-1 text-sm">Note<Input autoFocus placeholder="Review this" maxLength={160} value={label} onChange={event => setLabel(event.target.value)} disabled={busy} /></label>
      <div className="grid gap-1 text-sm">
        <label htmlFor="bookmark-time">Time</label>
        <Input id="bookmark-time" autoComplete="off" placeholder="MM:SS" value={time} disabled={busy} aria-invalid={invalidTime} aria-describedby="bookmark-time-help" onChange={event => setTime(event.target.value)} />
        <span id="bookmark-time-help" className={invalidTime ? 'text-xs text-destructive' : 'text-xs text-muted-foreground'}>
          {invalidTime ? 'Enter a time within seven days as MM:SS, HH:MM:SS, or seconds.' : 'Enter MM:SS, HH:MM:SS, or seconds. Times past the end use the recording duration.'}
        </span>
      </div>
      <div className="flex flex-wrap justify-end gap-2">
        {currentTime !== undefined && <Button type="button" size="sm" variant="outline" disabled={busy} onClick={() => setTime(currentTime.toFixed(1))}>Use playback time</Button>}
        <Button type="submit" size="sm" disabled={busy || seconds === null}>{busy ? 'Saving…' : 'Add bookmark'}</Button>
      </div>
    </form>
  );
  return <>
    <Dialog open={addOpen} onOpenChange={open => { if (!busy) onAddOpenChange?.(open); }}><DialogContent onCloseAutoFocus={event => {
      if (onReturnFocus) { event.preventDefault(); onReturnFocus(); }
    }}><DialogHeader><DialogTitle>Add bookmark</DialogTitle><DialogDescription>Save a moment and an optional note to this meeting. Notes are private by default.</DialogDescription></DialogHeader>{addForm}</DialogContent></Dialog>
    {error && <p role="alert" className="mt-2 text-destructive">{error} <Button variant="ghost" className="h-auto gap-0 rounded-none p-0 text-sm font-normal underline hover:bg-transparent hover:text-current" onClick={() => setRevision(r => r + 1)}>Retry</Button></p>}
    {bookmarks.length > 0 && <Accordion type="single" collapsible value={section} onValueChange={setSection} className="border-b border-border px-3 py-2 text-sm"><AccordionItem value="bookmarks" className="group/details border-0">
    <AccordionTrigger className="cursor-pointer rounded-none py-0 font-medium hover:no-underline">Bookmarks ({bookmarks.length})</AccordionTrigger><AccordionContent forceMount className="p-0 group-data-[state=closed]/details:hidden">
    <ul className="mt-2 max-h-48 space-y-2 overflow-y-auto">{bookmarks.map(mark => <li key={mark.id} id={`bookmark-row-${mark.id}`} tabIndex={-1} className="flex flex-wrap items-center gap-1 focus-visible:ring-2 focus-visible:ring-ring">
      {editing === mark.id ? <>
        <Input aria-label="Bookmark label" maxLength={160} className="h-auto shadow-none text-sm md:text-sm min-w-0 flex-1 rounded border border-input bg-background p-1" value={editLabel} onChange={e => setEditLabel(e.target.value)} />
        <Button size="sm" disabled={busy || !editLabel.trim()} onClick={() => change('rename_meeting_bookmark', { id: mark.id, label: editLabel })}>Save</Button>
        <Button size="sm" variant="ghost" disabled={busy} onClick={() => setEditing(null)}>Cancel</Button>
      </> : <>
      <Button variant="ghost" className="block h-auto gap-0 rounded-none p-0 text-sm font-normal hover:bg-transparent hover:text-primary disabled:opacity-100 w-full truncate text-left text-primary underline disabled:text-muted-foreground" disabled={!onSeek} onClick={() => onSeek?.(mark.seconds)} title={`${bookmarkTime(mark.seconds)} · ${mark.label}`}>{bookmarkTime(mark.seconds)} · {mark.label}</Button>
      <Button size="sm" variant="ghost" disabled={busy} aria-label={`Rename bookmark ${mark.label}`} onClick={() => { setEditing(mark.id); setEditLabel(mark.label); }}>Rename</Button>
      <Button size="sm" variant="ghost" disabled={busy} aria-label={`Remove bookmark ${mark.label}`} onClick={() => change('delete_meeting_bookmark', { id: mark.id })}>Remove</Button>
      </>}
    </li>)}</ul>
    {!onSeek && <p className="mt-2 text-xs text-muted-foreground">Audio playback is unavailable for this meeting.</p>}
    <label className="mt-3 flex items-start gap-2 text-xs text-muted-foreground">
      <Switch aria-label="Use bookmarks as summary priorities" checked={usePriorities === true} disabled={busy || usePriorities === null} onCheckedChange={setSummaryPriorities} />
      <span><span className="block font-medium text-foreground">Use as summary priorities</span>Sends the first 40 notes in timestamp order to the summary model, including for automatic summaries. Off by default.</span>
    </label>
    {prioritiesError && <p role="alert" className="mt-2 text-destructive">Could not load the summary preference. <Button variant="ghost" className="h-auto p-0 underline" onClick={() => setRevision(r => r + 1)}>Retry</Button></p>}
  </AccordionContent></AccordionItem></Accordion>}
  </>;
}
