'use client';

import { useEffect, useRef, useState, type FormEvent } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { PencilLine } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { Switch } from '@/components/ui/switch';
import { bookmarkTime, type MeetingBookmark } from '@/lib/library';

interface LiveRecordingNotesProps {
  sessionId: string | null;
  disabled: boolean;
  className?: string;
}

interface LiveRecordingBookmarks {
  bookmarks: MeetingBookmark[];
  include_in_summary: boolean;
}

export function LiveRecordingNotes({ sessionId, disabled, className }: LiveRecordingNotesProps) {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState('');
  const [snapshot, setSnapshot] = useState<LiveRecordingBookmarks | null>(null);
  const [loadError, setLoadError] = useState(false);
  const [revision, setRevision] = useState(0);
  const [busy, setBusy] = useState(false);
  const saving = useRef(false);

  useEffect(() => {
    if (!open || !sessionId) return;
    let active = true;
    setLoadError(false);
    void invoke<LiveRecordingBookmarks>('list_live_recording_bookmarks', { sessionId }).then(result => {
      if (active) setSnapshot(result);
    }).catch(() => {
      if (active) setLoadError(true);
    });
    return () => { active = false; };
  }, [open, sessionId, revision]);

  const save = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const label = draft.trim();
    if (!sessionId || disabled || !label || saving.current) return;
    saving.current = true;
    setBusy(true);
    try {
      await invoke('add_meeting_bookmark', { meetingId: null, seconds: null, sessionId, label });
      setDraft('');
      setRevision(value => value + 1);
      toast.success('Note saved');
    } catch (error) {
      toast.error('Could not save note', { description: String(error) });
    } finally {
      saving.current = false;
      setBusy(false);
    }
  };

  const setSummaryConsent = async (enabled: boolean) => {
    if (!sessionId || disabled || saving.current) return;
    saving.current = true;
    setBusy(true);
    try {
      await invoke('set_live_summary_bookmarks', { sessionId, enabled });
      setSnapshot(previous => previous ? { ...previous, include_in_summary: enabled } : previous);
      setRevision(value => value + 1);
    } catch {
      toast.error('Could not save the summary preference');
    } finally {
      saving.current = false;
      setBusy(false);
    }
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button variant="ghost" type="button" className={className} aria-label="Add timestamped note" disabled={!sessionId || disabled}>
          <PencilLine size={16} />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-80 space-y-3">
        <h2 className="font-medium">My notes</h2>
        <form className="flex items-end gap-2" onSubmit={save}>
          <label className="grid min-w-0 flex-1 gap-1 text-sm">Note
            <Input maxLength={160} value={draft} onChange={event => setDraft(event.target.value)} readOnly={busy} />
          </label>
          <Button type="submit" disabled={!sessionId || disabled || busy || !draft.trim()}>Save note</Button>
        </form>
        {loadError ? (
          <p role="alert" className="text-sm text-destructive">Could not load notes. <Button variant="ghost" className="h-auto p-0 underline" onClick={() => setRevision(value => value + 1)}>Retry</Button></p>
        ) : snapshot === null ? (
          <p role="status" className="text-sm text-muted-foreground">Loading notes…</p>
        ) : snapshot.bookmarks.length === 0 ? (
          <p className="text-sm text-muted-foreground">Notes use the current recording time and stay in the local library.</p>
        ) : (
          <ul aria-label="Recent notes" className="max-h-44 space-y-2 overflow-y-auto text-sm">
            {snapshot.bookmarks.map(note => <li key={note.id} className="flex gap-2"><span className="shrink-0 tabular-nums text-muted-foreground">{bookmarkTime(note.seconds)}</span><span className="min-w-0 break-words">{note.label}</span></li>)}
          </ul>
        )}
        <label className="flex items-start gap-2 border-t border-border pt-3 text-xs text-muted-foreground">
          <Switch aria-label="Use notes as summary priorities" checked={snapshot?.include_in_summary === true} disabled={!snapshot || loadError || disabled || busy} onCheckedChange={setSummaryConsent} />
          <span><span className="block font-medium text-foreground">Use as summary priorities</span>Sends the first 40 notes in timestamp order to the summary model, including for automatic summaries. Off by default.</span>
        </label>
      </PopoverContent>
    </Popover>
  );
}
