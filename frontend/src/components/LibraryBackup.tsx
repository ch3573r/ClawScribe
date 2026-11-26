'use client';
import { useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { flushMeetingContexts } from '@/lib/meetingContext';
import { migrateLegacyMeetingCalendars } from '@/lib/meetingCalendarStore';
import type { LibraryArchiveReport } from '@/lib/library';
import { ScheduledBackups } from '@/components/ScheduledBackups';

const audioExcludedNotice = 'This backup does not include recording audio.';

export function LibraryBackup() {
  const [visible, setVisible] = useState(false);
  const [busy, setBusy] = useState('');
  const [result, setResult] = useState<{ report: LibraryArchiveReport; restore: boolean } | null>(null);
  const pending = useRef(false);
  const { isRecording, isStarting, isStopping, isProcessing, isSaving } = useRecordingState();
  const { refetchMeetings, refreshProjectTags } = useSidebar();
  const unavailable = isRecording || isStarting || isStopping || isProcessing || isSaving;
  const skippedTemplates = result?.report.skipped_template_items ?? 0;
  const omittedAnswers = result?.report.omitted_saved_live_answers ?? 0;
  const incompleteMeetings = result?.report.incomplete_meetings ?? [];
  const run = async (restore: boolean) => {
    if (pending.current) return;
    pending.current = true; setBusy(restore ? 'Restoring…' : 'Backing up…');
    try {
      // The core opens the dialog and only accepts the path it chose here.
      const path = restore
        ? await invoke<string | null>('pick_backup_restore_path')
        : await invoke<string | null>('pick_backup_save_path', { suggestedName: `ClawScribe-backup-${new Date().toISOString().slice(0, 10)}.zip` });
      if (!path) return;
      setResult(null);
      await migrateLegacyMeetingCalendars();
      if (!restore) await flushMeetingContexts();
      const report = await invoke<LibraryArchiveReport>(restore ? 'restore_library' : 'backup_library', { path });
      setResult({ report, restore });
      const incomplete = report.incomplete_meetings?.length ?? 0;
      const unreadable = report.unreadable_calendar_links ?? 0;
      const templatesSkipped = report.skipped_template_items ?? 0;
      const answersOmitted = report.omitted_saved_live_answers ?? 0;
      const description = `${report.meetings} meetings, ${report.files} recording files.${report.audio_excluded ? ` ${audioExcludedNotice}` : ''}${report.skipped ? ` ${report.skipped} existing meetings skipped.` : ''}${unreadable ? ` ${unreadable} saved calendar links could not be read and were left unlinked.` : ''}${incomplete ? ` ${incomplete} meetings have incomplete audio; review the list below.` : ''}${answersOmitted ? ` Excluded ${answersOmitted} saved live ${answersOmitted === 1 ? 'answer' : 'answers'}.` : ''}`;
      if (incomplete) toast.warning(restore ? 'Restore complete with incomplete audio' : 'Backup saved with incomplete audio', { description });
      else if (templatesSkipped) toast.warning(restore ? 'Restore complete with templates skipped' : 'Backup saved with templates skipped', { description });
      else if (answersOmitted) toast.warning(restore ? 'Restore complete with saved answers excluded' : 'Backup saved with saved answers excluded', { description });
      else toast.success(restore ? 'Restore complete' : 'Backup saved', { description });
      if (restore) {
        const refreshed = await Promise.allSettled([refetchMeetings(), refreshProjectTags()]);
        if (refreshed.some(item => item.status === 'rejected')) toast.error('Restore saved, but the meeting list could not refresh. Reopen the library.');
      }
    } catch (error) { toast.error(restore ? 'Restore failed' : 'Backup failed', { description: String(error) }); }
    finally { pending.current = false; setBusy(''); }
  };
  return <>
    <Button variant="outline" onClick={() => setVisible(true)}>Backup and restore</Button>
    <Dialog open={visible} onOpenChange={value => { if (!busy) setVisible(value); }}><DialogContent className="max-h-[calc(100dvh-2rem)] overflow-y-auto">
      <DialogHeader><DialogTitle>Backup and restore</DialogTitle><DialogDescription>Save meetings, recordings, transcript history, notes, custom templates, vocabulary, and saved series preferences.</DialogDescription></DialogHeader>
      <p className="text-sm text-muted-foreground">Archives contain private content. Store these unencrypted files in a trusted folder.</p>
      <p className="text-sm text-muted-foreground">Restore adds missing meetings and keeps your existing preferences. Finish recording and transcription jobs first.</p>
      {result && (result.report.audio_excluded || incompleteMeetings.length > 0 || skippedTemplates > 0 || omittedAnswers > 0) && <div role="alert" className="space-y-2 rounded border border-warning-border/40 bg-warning p-3 text-sm">
        {result.report.audio_excluded && <p>{audioExcludedNotice}</p>}
        {skippedTemplates > 0 && <p>Skipped {skippedTemplates} template {skippedTemplates === 1 ? 'item' : 'items'} that could not be read or saved.</p>}
        {omittedAnswers > 0 && <p>{omittedAnswers === 1 ? '1 saved live answer was' : `${omittedAnswers} saved live answers were`} excluded.</p>}
        {incompleteMeetings.length > 0 && <>
          <p className="font-medium">{result.restore ? 'Restore complete with incomplete audio' : 'Backup saved with incomplete audio'}</p>
          <p>{result.report.audio_excluded ? 'Meeting data is included.' : 'Meeting data and available saved audio are included.'} If recovery files were excluded, keep a separate copy of the source recording folders, including hidden files.</p>
          <ul className="list-disc space-y-1 pl-5">{incompleteMeetings.map(meeting => <li key={meeting.meeting_id}>
            <span className="font-medium">{meeting.title || 'Untitled meeting'}</span>: {[
              meeting.recovery_files_excluded && 'recovery files excluded',
              meeting.recording_folder_missing && 'recording folder missing',
              meeting.audio_unavailable && 'saved audio unavailable',
            ].filter(Boolean).join('; ')}.
          </li>)}</ul>
        </>}
      </div>}
      <div className="flex flex-wrap gap-2"><Button disabled={!!busy || unavailable} onClick={() => run(false)}>Save backup</Button><Button variant="outline" disabled={!!busy || unavailable} onClick={() => run(true)}>Choose archive to restore</Button></div>
      {busy && <p role="status" className="text-sm">{busy} Keep ClawScribe open until this finishes.</p>}
      {unavailable && <p role="status" className="text-sm">Finish the active recording before using backup or restore.</p>}
      {visible && <ScheduledBackups disabled={!!busy || unavailable} />}
    </DialogContent></Dialog>
  </>;
}
