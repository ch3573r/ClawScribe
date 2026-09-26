'use client';
import { useState, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from '@/components/ui/dialog';
import { parseProjectTags } from '@/lib/library';

export function ProjectTags({ meetingId }: { meetingId: string }) {
  const { projectTags, projectTagsError, refreshProjectTags } = useSidebar();
  const tags = projectTags.filter(tag => tag.meeting_id === meetingId).map(tag => tag.tag);
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const save = async () => {
    if (pending.current) return;
    pending.current = true; setBusy(true);
    try {
      await invoke('set_meeting_tags', { meetingId, tags: parseProjectTags(draft) });
      await refreshProjectTags(); setOpen(false); toast.success('Project tags saved');
    } catch (error) { toast.error('Could not save tags', { description: String(error) }); }
    finally { pending.current = false; setBusy(false); }
  };
  return <>
    <Button variant="outline" size="sm" onClick={() => { if (projectTagsError) { void refreshProjectTags(); return; } setDraft(tags.join(', ')); setOpen(true); }}>
      {projectTagsError ? 'Retry loading tags' : tags.length ? `Tags: ${tags.join(', ')}` : 'Project tags'}
    </Button>
    <Dialog open={open} onOpenChange={value => { if (!busy) setOpen(value); }}><DialogContent>
      <DialogHeader><DialogTitle>Project tags</DialogTitle><DialogDescription>Separate tags with commas. Use up to 20 tags, each up to 60 characters. Clear the field to remove all tags.</DialogDescription></DialogHeader>
      <label className="grid gap-2 text-sm">Tags<input className="rounded-md border border-input bg-background p-2" value={draft} onChange={e => setDraft(e.target.value)} disabled={busy} placeholder="Project Atlas, Customer meetings" /></label>
      <DialogFooter><Button disabled={busy} onClick={save}>{busy ? 'Saving…' : 'Save tags'}</Button></DialogFooter>
    </DialogContent></Dialog>
  </>;
}
