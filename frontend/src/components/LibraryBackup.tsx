'use client';
import { useRef, useState } from 'react';
import { open, save } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';

export function LibraryBackup() {
  const [visible, setVisible] = useState(false);
  const [busy, setBusy] = useState('');
  const pending = useRef(false);
  const { isRecording, isStarting, isStopping, isProcessing, isSaving } = useRecordingState();
  const { refetchMeetings, refreshProjectTags } = useSidebar();
  const unavailable = isRecording || isStarting || isStopping || isProcessing || isSaving;
  const run = async (restore: boolean) => {
    if (pending.current) return;
    pending.current = true; setBusy(restore ? 'Restoring…' : 'Backing up…');
    try {
      const filters = [{ name: 'ClawScribe meeting archive', extensions: ['zip'] }];
      const path = restore ? await open({ filters, multiple: false, directory: false }) : await save({ filters, defaultPath: `ClawScribe-backup-${new Date().toISOString().slice(0, 10)}.zip` });
      if (!path) return;
      const report = await invoke<{ meetings: number; skipped: number; files: number }>(restore ? 'restore_library' : 'backup_library', { path });
      await Promise.all([refetchMeetings(), refreshProjectTags()]);
      toast.success(restore ? 'Restore complete' : 'Backup saved', { description: `${report.meetings} meetings, ${report.files} recording files.${report.skipped ? ` ${report.skipped} existing meetings skipped.` : ''}` });
    } catch (error) { toast.error(restore ? 'Restore failed' : 'Backup failed', { description: String(error) }); }
    finally { pending.current = false; setBusy(''); }
  };
  return <>
    <Button variant="outline" onClick={() => setVisible(true)}>Backup and restore</Button>
    <Dialog open={visible} onOpenChange={value => { if (!busy) setVisible(value); }}><DialogContent>
      <DialogHeader><DialogTitle>Backup and restore</DialogTitle><DialogDescription>Keep a portable copy of saved meetings, recordings, transcripts, notes, project tags, and bookmarks.</DialogDescription></DialogHeader>
      <p className="text-sm text-muted-foreground">Archives contain private meeting content and are not encrypted. Store them somewhere you trust. Models, provider settings, and sign-in credentials are excluded.</p>
      <p className="text-sm text-muted-foreground">Restore adds missing meetings. Meetings with an existing ID are skipped, so your current library is preserved. Stop recording and transcription jobs first.</p>
      <div className="flex flex-wrap gap-2"><Button disabled={!!busy || unavailable} onClick={() => run(false)}>Save backup</Button><Button variant="outline" disabled={!!busy || unavailable} onClick={() => run(true)}>Choose archive to restore</Button></div>
      {busy && <p role="status" className="text-sm">{busy} Keep ClawScribe open until this finishes.</p>}
      {unavailable && <p role="status" className="text-sm">Finish the active recording before using backup or restore.</p>}
    </DialogContent></Dialog>
  </>;
}
