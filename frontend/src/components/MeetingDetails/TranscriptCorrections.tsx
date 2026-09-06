'use client';
import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { Textarea } from '@/components/ui/textarea';
import { ReplaceOptions, ReplacePreview, TextReplaceDialog } from './TextReplaceDialog';
import { TranscriptSegmentData } from '@/types';

interface EditResult { changed: number; file_warning: boolean }
interface EditState { can_undo: boolean; pending_file_sync: boolean; has_edits: boolean }

export function TranscriptCorrections({ meetingId, editing, onClose, onChanged, replaceOpen, onReplaceOpenChange }: {
  meetingId: string; editing: TranscriptSegmentData | null; onClose: () => void; onChanged?: () => Promise<void>;
  replaceOpen: boolean; onReplaceOpenChange: (open: boolean) => void;
}) {
  const [text, setText] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [state, setState] = useState<EditState>({ can_undo: false, pending_file_sync: false, has_edits: false });
  const operation = useRef(false);
  const currentMeeting = useRef(meetingId);
  currentMeeting.current = meetingId;
  useEffect(() => { setText(editing?.text ?? ''); setError(''); }, [editing]);
  const refresh = useCallback(async () => {
    const value = await invoke<EditState>('api_get_transcript_edit_state', { meetingId });
    if (currentMeeting.current === meetingId) setState(value);
  }, [meetingId]);
  useEffect(() => {
    currentMeeting.current = meetingId;
    setState({ can_undo: false, pending_file_sync: false, has_edits: false });
    void refresh().catch(() => setError('Could not load correction history. Reopen the meeting to retry.'));
    const subscription = listen<{meeting_id: string}>('transcript-text-edited', event => { if (event.payload.meeting_id === meetingId) void refresh().catch(() => {}); });
    return () => { currentMeeting.current = ''; void subscription.then(unlisten => unlisten()).catch(() => {}); };
  }, [meetingId, refresh]);

  const completed = async (result: EditResult) => {
    if (currentMeeting.current !== meetingId) return;
    setState(previous => ({ ...previous, pending_file_sync: result.file_warning }));
    toast.success(`${result.changed} transcript ${result.changed === 1 ? 'passage' : 'passages'} updated`, { description: 'Regenerate meeting notes to use the corrected text.' });
    try { await refresh(); await onChanged?.(); }
    catch { setError('The correction is saved. Reopen the meeting to refresh its transcript.'); }
  };
  const change = async (command: string, args: Record<string, unknown>) => {
    if (operation.current) return;
    operation.current = true;
    setBusy(true); setError('');
    try { const result = await invoke<EditResult>(command, { meetingId, ...args }); await completed(result); if (currentMeeting.current === meetingId) onClose(); }
    catch (e) { if (currentMeeting.current === meetingId) setError(typeof e === 'string' ? e : 'Could not save the correction. Your text is retained.'); }
    finally { operation.current = false; setBusy(false); }
  };
  const preview = (options: ReplaceOptions) => invoke<ReplacePreview>('api_preview_transcript_replace', { meetingId, ...options });
  const replace = async (options: ReplaceOptions, token: string) => {
    const result = await invoke<EditResult>('api_replace_transcript_text', { meetingId, ...options, previewToken: token });
    await completed(result);
  };
  return <>
    <div className="space-y-2 border-b border-border px-3 py-2 text-xs">
      <div className="flex flex-wrap gap-2"><Button size="sm" variant="outline" disabled={busy} onClick={() => onReplaceOpenChange(true)}>Find and replace</Button>
        <Button size="sm" variant="ghost" disabled={busy || !state.can_undo} onClick={() => void change('api_undo_transcript_edit', {})}>Undo last correction</Button></div>
      {state.has_edits && <p className="text-muted-foreground">Transcript corrected. Regenerate notes to include the changes.</p>}
      {state.pending_file_sync && <p role="alert">Corrections are saved in ClawScribe. The recording-folder copy needs updating. <button className="underline" disabled={busy} onClick={async () => {
        setBusy(true); setError('');
        try { await invoke('api_sync_transcript_file', { meetingId }); await refresh(); }
        catch { setError('Could not update the recording-folder copy. Check that the folder is available, then retry.'); }
        finally { setBusy(false); }
      }}>Retry file update</button></p>}
      {!editing && error && <p role="alert" className="text-destructive">{error}</p>}
    </div>
    <Dialog open={Boolean(editing)} onOpenChange={open => { if (!open && !busy) onClose(); }}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-xl"><DialogHeader><DialogTitle>Correct transcript</DialogTitle><DialogDescription>Keep the spoken meaning. The audio and segment timestamps stay unchanged; undo restores the previous text.</DialogDescription></DialogHeader>
        <label className="space-y-2 text-sm">Transcript text<Textarea autoFocus rows={8} value={text} disabled={busy} onChange={e => setText(e.target.value)} /></label>
        {editing?.original_text && <details className="text-sm"><summary className="cursor-pointer">Original recognition</summary><p className="mt-2 whitespace-pre-wrap break-words text-muted-foreground">{editing.original_text}</p></details>}
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        <DialogFooter><Button variant="outline" disabled={busy} onClick={onClose}>Cancel</Button><Button disabled={busy || !text.trim() || text === editing?.text} onClick={() => editing && void change('api_update_transcript_text', { transcriptId: editing.id, expectedText: editing.text, text })}>{busy ? 'Saving…' : 'Save correction'}</Button></DialogFooter>
      </DialogContent>
    </Dialog>
    <TextReplaceDialog key={meetingId} open={replaceOpen} onOpenChange={onReplaceOpenChange} title="transcript" preview={preview} apply={replace} />
  </>;
}
