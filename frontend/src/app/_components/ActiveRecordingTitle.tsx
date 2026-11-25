import { useRef, useState, type FormEvent } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Pencil } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';

export function ActiveRecordingTitle({ sessionId, onSaved }: {
  sessionId: string;
  onSaved: (sessionId: string, title: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [title, setTitle] = useState('');
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const request = useRef(0);
  const pending = useRef(false);

  async function load() {
    const version = ++request.current;
    setLoading(true);
    setError('');
    try {
      const title = await invoke<string>('get_active_recording_title', { sessionId });
      if (version === request.current) setTitle(title);
    } catch {
      if (version === request.current) setError('Could not load the meeting title.');
    } finally {
      if (version === request.current) setLoading(false);
    }
  }

  function changeOpen(next: boolean) {
    if (pending.current) return;
    setOpen(next);
    if (next) {
      setTitle('');
      void load();
    } else {
      request.current++;
    }
  }

  async function save(event: FormEvent) {
    event.preventDefault();
    if (pending.current || loading || !title.trim()) return;
    pending.current = true;
    setSaving(true);
    setError('');
    try {
      const savedTitle = await invoke<string>('set_active_recording_title', { sessionId, title });
      onSaved(sessionId, savedTitle);
      setOpen(false);
    } catch (error) {
      setError(typeof error === 'string' ? error : 'Could not save the meeting title. Try again.');
    } finally {
      pending.current = false;
      setSaving(false);
    }
  }

  return <Dialog open={open} onOpenChange={changeOpen}>
    <DialogTrigger asChild><Button variant="ghost" size="sm"><Pencil aria-hidden="true" />Edit meeting title</Button></DialogTrigger>
    <DialogContent>
      <DialogHeader>
        <DialogTitle>Edit meeting title</DialogTitle>
        <DialogDescription>This title will appear in your library when you save this recording.</DialogDescription>
      </DialogHeader>
      <form onSubmit={save} className="space-y-4">
        <div className="space-y-1.5">
          <label htmlFor="active-meeting-title" className="text-sm">Meeting title</label>
          <Input id="active-meeting-title" value={title} maxLength={160} readOnly={loading || saving}
            onChange={event => setTitle(event.target.value)} />
        </div>
        {loading && <p role="status" className="text-sm text-muted-foreground">Loading title…</p>}
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        <DialogFooter>
          <Button type="button" variant="outline" disabled={saving} onClick={() => changeOpen(false)}>Cancel</Button>
          <Button type="submit" disabled={loading || saving || !title.trim()}>{saving ? 'Saving…' : 'Save title'}</Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>;
}
