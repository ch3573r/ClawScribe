"use client";
import { useEffect, useRef, useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { Paperclip } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { knowledgeService } from '@/services/knowledgeService';
import { referenceError } from '@/lib/knowledge-documents';

export interface AttachmentMeeting { id: string; title: string }

export function AttachReferenceButton({ meetingId, meetings = [], scopeKey, disabled = false, onAttached, onBusyChange }: {
  meetingId?: string; meetings?: AttachmentMeeting[]; scopeKey?: string; disabled?: boolean;
  onAttached: (meetingId: string) => void;
  onBusyChange?: (busy: boolean) => void;
}) {
  const [pickerOpen, setPickerOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [busy, setBusy] = useState(false);
  const [extracting, setExtracting] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [error, setError] = useState('');
  const pending = useRef(false);
  const cancelPending = useRef(false);
  const activeImport = useRef<string | null>(null);
  const generation = useRef(0);
  const mounted = useRef(false);
  const identity = JSON.stringify([meetingId, scopeKey, meetings.map(meeting => meeting.id)]);
  useEffect(() => {
    mounted.current = true;
    generation.current++;
    setPickerOpen(false); setQuery(''); setError('');
    return () => {
      mounted.current = false;
      generation.current++;
      if (activeImport.current) void knowledgeService.cancelDocumentImport(activeImport.current).catch(() => {});
    };
  }, [identity]);

  async function attach(target: string) {
    if (pending.current || disabled) return;
    pending.current = true; setBusy(true); onBusyChange?.(true); setError(''); setPickerOpen(false);
    const current = generation.current;
    let request: string | null = null;
    try {
      const path = await open({ multiple: false, directory: false,
        filters: [{ name: 'Reference documents', extensions: ['pdf', 'docx', 'txt', 'md'] }] });
      if (current !== generation.current || typeof path !== 'string') return;
      request = globalThis.crypto.randomUUID(); activeImport.current = request; setExtracting(true);
      await knowledgeService.importDocument(target, path, request);
      if (current === generation.current) onAttached(target);
    } catch (failure) {
      if (current === generation.current) setError(referenceError(failure));
    } finally {
      if (activeImport.current === request) activeImport.current = null;
      pending.current = false; cancelPending.current = false;
      if (mounted.current) { setBusy(false); onBusyChange?.(false); setExtracting(false); setCancelling(false); }
    }
  }
  function choose() {
    if (pending.current || disabled) return;
    if (meetingId) return attach(meetingId);
    setError(''); setQuery(''); setPickerOpen(true);
  }
  async function cancel() {
    const request = activeImport.current;
    if (!request || cancelPending.current) return;
    cancelPending.current = true; setCancelling(true);
    const current = generation.current;
    try { await knowledgeService.cancelDocumentImport(request); }
    catch (failure) {
      if (current === generation.current) { setError(referenceError(failure)); setCancelling(false); }
      cancelPending.current = false;
    }
  }
  const available = !!meetingId || meetings.length > 0;
  const matching = meetings.filter(meeting => meeting.title.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()));
  return <div className="space-y-2">
    <div className="flex flex-wrap items-center gap-2">
      <Button type="button" variant="outline" size="sm" disabled={disabled || busy || !available} onClick={choose}>
        <Paperclip />{busy ? extracting ? 'Attaching…' : 'Choosing file…' : 'Attach file'}
      </Button>
      {extracting && <><p role="status" className="text-xs text-muted-foreground">{cancelling ? 'Cancelling extraction…' : 'Extracting locally…'}</p>
        <Button type="button" variant="ghost" size="sm" disabled={cancelling} onClick={() => void cancel()}>Cancel extraction</Button></>}
    </div>
    {!available && <p className="text-xs text-muted-foreground">Save a meeting or adjust your filters before attaching a file.</p>}
    {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
    <Dialog open={pickerOpen} onOpenChange={setPickerOpen}><DialogContent><DialogHeader>
      <DialogTitle>Attach file</DialogTitle><DialogDescription>Choose the saved meeting to attach this reference to. Its files will be available in Meeting memory.</DialogDescription>
    </DialogHeader>
      <Input autoFocus aria-label="Find an attachment meeting" value={query} onChange={event => setQuery(event.target.value)} placeholder="Find a meeting…" />
      <ScrollArea className="h-56"><div className="space-y-1 pr-3">
        {matching.map(meeting => <Button type="button" key={meeting.id} variant="ghost" className="h-auto w-full justify-start whitespace-normal text-left" onClick={() => void attach(meeting.id)}>{meeting.title}</Button>)}
        {!matching.length && <p className="text-sm text-muted-foreground">No matching meetings.</p>}
      </div></ScrollArea>
    </DialogContent></Dialog>
  </div>;
}
