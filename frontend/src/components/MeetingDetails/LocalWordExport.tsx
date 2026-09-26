'use client';
import { useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter } from '@/components/ui/dialog';
import { safeDocumentName } from '@/lib/library';

export function LocalWordExport({ meetingId, title, getMarkdown, disabled }: { meetingId: string; title: string; getMarkdown: () => Promise<string>; disabled?: boolean }) {
  const [open, setOpen] = useState(false);
  const [content, setContent] = useState('both');
  const [speakers, setSpeakers] = useState(true);
  const [timestamps, setTimestamps] = useState(true);
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const exportDocument = async () => {
    if (pending.current) return;
    pending.current = true; setBusy(true);
    try {
      const markdown = content === 'transcript' ? '' : await getMarkdown();
      if (content === 'summary' && !markdown.trim()) throw new Error('Generate or save notes before exporting a summary.');
      const path = await save({ defaultPath: safeDocumentName(title), filters: [{ name: 'Word document', extensions: ['docx'] }] });
      if (!path) return;
      await invoke('export_local_word', { meetingId, title, markdown, includeTranscript: content !== 'summary', includeSpeakers: speakers, includeTimestamps: timestamps, path });
      toast.success('Word document saved'); setOpen(false);
    } catch (error) { toast.error('Word export failed', { description: String(error) }); }
    finally { pending.current = false; setBusy(false); }
  };
  return <>
    <Button variant="outline" size="sm" disabled={disabled} onClick={() => setOpen(true)}>Export Word</Button>
    <Dialog open={open} onOpenChange={value => { if (!busy) setOpen(value); }}>
      <DialogContent><DialogHeader><DialogTitle>Export Word document</DialogTitle><DialogDescription>Save an editable .docx on this computer. Microsoft sign-in is not required.</DialogDescription></DialogHeader>
        <label className="grid gap-2 text-sm">Include
          <select className="rounded-md border border-input bg-background p-2" value={content} onChange={e => setContent(e.target.value)} disabled={busy}>
            <option value="both">Summary and transcript</option><option value="summary">Summary only</option><option value="transcript">Transcript only</option>
          </select>
        </label>
        <label className="flex items-center gap-2 text-sm"><input className="accent-[hsl(var(--primary))]" type="checkbox" checked={speakers} disabled={busy || content === 'summary'} onChange={e => setSpeakers(e.target.checked)} />Speaker labels</label>
        <label className="flex items-center gap-2 text-sm"><input className="accent-[hsl(var(--primary))]" type="checkbox" checked={timestamps} disabled={busy || content === 'summary'} onChange={e => setTimestamps(e.target.checked)} />Transcript timestamps</label>
        <DialogFooter><Button disabled={busy} onClick={exportDocument}>{busy ? 'Saving…' : 'Save as Word document'}</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  </>;
}
