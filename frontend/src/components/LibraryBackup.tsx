'use client';
import { useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { flushMeetingContexts } from '@/lib/meetingContext';
import { migrateLegacyMeetingCalendars } from '@/lib/meetingCalendarStore';
import type { LibraryArchiveReport } from '@/lib/library';
import { ScheduledBackups } from '@/components/ScheduledBackups';

const audioExcludedNotice = 'This backup does not include recording audio.';
const passphraseRequired = 'Enter the passphrase for this backup.';

export function LibraryBackup() {
  const [visible, setVisible] = useState(false);
  const [busy, setBusy] = useState('');
  const [encrypt, setEncrypt] = useState(false);
  const [repairMissingAudio, setRepairMissingAudio] = useState(false);
  const [backupPassphrase, setBackupPassphrase] = useState('');
  const [confirmation, setConfirmation] = useState('');
  const [restorePath, setRestorePath] = useState<string | null>(null);
  const [restorePassphrase, setRestorePassphrase] = useState('');
  const [result, setResult] = useState<{ report: LibraryArchiveReport; restore: boolean } | null>(null);
  const pending = useRef(false);
  const { isRecording, isStarting, isStopping, isProcessing, isSaving } = useRecordingState();
  const { refetchMeetings, refreshProjectTags } = useSidebar();
  const unavailable = isRecording || isStarting || isStopping || isProcessing || isSaving;
  const skippedTemplates = result?.report.skipped_template_items ?? 0;
  const omittedAnswers = result?.report.omitted_saved_live_answers ?? 0;
  const incompleteMeetings = result?.report.incomplete_meetings ?? [];
  const unavailableLocations = result?.report.unavailable_audio_locations ?? 0;
  const unavailableLocationNotice = (count: number) =>
    `Recording locations were unavailable for ${count} ${count === 1 ? 'meeting' : 'meetings'}. Reconnect the storage and retry repair.`;
  const passphrasesMatch = backupPassphrase === confirmation;
  const canSave = !encrypt || (!!backupPassphrase.trim() && passphrasesMatch);
  const clearPassphrases = () => {
    setBackupPassphrase('');
    setConfirmation('');
    setRestorePassphrase('');
  };
  const run = async (restore: boolean, retryPath?: string) => {
    if (pending.current || unavailable || (!restore && !canSave)) return;
    const passphrase = restore
      ? (retryPath ? restorePassphrase : undefined)
      : (encrypt ? backupPassphrase : undefined);
    clearPassphrases();
    pending.current = true;
    setBusy(restore ? 'Restoring…' : 'Backing up…');
    let path: string | null | undefined = retryPath;
    try {
      // The core opens the dialog and only accepts the path it chose here.
      path ??= restore
        ? await invoke<string | null>('pick_backup_restore_path')
        : await invoke<string | null>('pick_backup_save_path', {
          encrypted: encrypt,
          suggestedName: `ClawScribe-backup-${new Date().toISOString().slice(0, 10)}.${encrypt ? 'clawbackup' : 'zip'}`,
        });
      if (!path) return;
      setResult(null);
      await migrateLegacyMeetingCalendars();
      if (!restore) await flushMeetingContexts();
      const report = restore
        ? await invoke<LibraryArchiveReport>('restore_library', { path, passphrase, repairMissingAudio })
        : await invoke<LibraryArchiveReport>('backup_library', { path, passphrase });
      setRestorePath(null);
      if (restore) setRepairMissingAudio(false);
      setResult({ report, restore });
      const incomplete = report.incomplete_meetings?.length ?? 0;
      const unreadable = report.unreadable_calendar_links ?? 0;
      const templatesSkipped = report.skipped_template_items ?? 0;
      const answersOmitted = report.omitted_saved_live_answers ?? 0;
      const repaired = report.repaired_audio ?? 0;
      const description = [
        `${report.meetings} meetings${restore ? ' added' : ''}, ${report.files} files.`,
        repaired && `Restored missing audio for ${repaired} ${repaired === 1 ? 'meeting' : 'meetings'}.`,
        !!report.unavailable_audio_locations && unavailableLocationNotice(report.unavailable_audio_locations),
        report.audio_excluded && audioExcludedNotice,
        report.skipped && `${report.skipped} existing meetings skipped.`,
        unreadable && `${unreadable} saved calendar links could not be read and were left unlinked.`,
        incomplete && `${incomplete} meetings have incomplete audio. Review the list below.`,
        answersOmitted && `Excluded ${answersOmitted} saved live ${answersOmitted === 1 ? 'answer' : 'answers'}.`,
      ].filter(Boolean).join(' ');
      if (incomplete) toast.warning(restore ? 'Restore complete with incomplete audio' : 'Backup saved with incomplete audio', { description });
      else if (report.unavailable_audio_locations) toast.warning('Restore complete with recording locations unavailable', { description });
      else if (templatesSkipped) toast.warning(restore ? 'Restore complete with templates skipped' : 'Backup saved with templates skipped', { description });
      else if (answersOmitted) toast.warning(restore ? 'Restore complete with saved answers excluded' : 'Backup saved with saved answers excluded', { description });
      else toast.success(restore ? 'Restore complete' : 'Backup saved', { description });
      if (restore) {
        const refreshed = await Promise.allSettled([refetchMeetings(), refreshProjectTags()]);
        if (refreshed.some(item => item.status === 'rejected')) toast.error('Restore saved, but the meeting list could not refresh. Reopen the library.');
      }
    } catch (error) {
      if (restore && path && String(error) === passphraseRequired) {
        setRestorePath(path);
      } else {
        toast.error(restore ? 'Restore failed' : 'Backup failed', { description: String(error) });
      }
    }
    finally { pending.current = false; setBusy(''); }
  };
  return <>
    <Button variant="outline" onClick={() => setVisible(true)}>Backup and restore</Button>
    <Dialog open={visible} onOpenChange={value => {
      if (busy) return;
      if (!value) {
        clearPassphrases();
        setEncrypt(false);
        setRepairMissingAudio(false);
        setRestorePath(null);
      }
      setVisible(value);
    }}><DialogContent className="max-h-[calc(100dvh-2rem)] overflow-y-auto">
      <DialogHeader><DialogTitle>Backup and restore</DialogTitle><DialogDescription>Save meetings, recordings, transcript history, notes, custom templates, vocabulary, and saved series preferences.</DialogDescription></DialogHeader>
      <p className="text-sm text-muted-foreground">Keep unencrypted archives in a trusted folder. Restore can add missing meetings and recover missing audio for existing meetings.</p>
      {result && (result.report.audio_excluded || incompleteMeetings.length > 0 || skippedTemplates > 0 || omittedAnswers > 0 || unavailableLocations > 0) && <div role="alert" className="space-y-2 rounded border border-warning-border/40 bg-warning p-3 text-sm">
        {result.report.audio_excluded && <p>{audioExcludedNotice}</p>}
        {unavailableLocations > 0 && <p>{unavailableLocationNotice(unavailableLocations)}</p>}
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
      {restorePath ? (
        <form className="space-y-3" onSubmit={event => { event.preventDefault(); if (restorePassphrase.trim()) return run(true, restorePath); }}>
          <label className="block space-y-1 text-sm font-medium">
            <span>Backup passphrase</span>
            <Input type="password" autoComplete="off" autoFocus value={restorePassphrase} maxLength={1024}
              disabled={!!busy || unavailable} onChange={event => setRestorePassphrase(event.target.value)} />
          </label>
          <div className="flex flex-wrap gap-2">
            <Button type="submit" disabled={!!busy || unavailable || !restorePassphrase.trim()}>Unlock and restore</Button>
            <Button type="button" variant="outline" disabled={!!busy} onClick={() => {
              setRestorePath(null);
              setRestorePassphrase('');
              setRepairMissingAudio(false);
            }}>Cancel</Button>
          </div>
        </form>
      ) : (
        <form className="space-y-3" onSubmit={event => { event.preventDefault(); return run(false); }}>
          <label className="flex items-center gap-2 text-sm font-medium">
            <Switch checked={encrypt} disabled={!!busy || unavailable} onCheckedChange={checked => {
              setEncrypt(checked);
              if (!checked) clearPassphrases();
            }} />
            Encrypt this backup
          </label>
          {encrypt && <>
            <p className="text-sm text-muted-foreground">You’ll need this passphrase to restore the backup. ClawScribe can’t recover it.</p>
            <label className="block space-y-1 text-sm font-medium">
              <span>Backup passphrase</span>
              <Input type="password" autoComplete="off" value={backupPassphrase} maxLength={1024}
                disabled={!!busy || unavailable} onChange={event => setBackupPassphrase(event.target.value)} />
            </label>
            <label className="block space-y-1 text-sm font-medium">
              <span>Confirm passphrase</span>
              <Input type="password" autoComplete="off" value={confirmation} maxLength={1024}
                disabled={!!busy || unavailable} onChange={event => setConfirmation(event.target.value)} />
            </label>
            {confirmation && !passphrasesMatch && <p role="status" className="text-sm text-destructive">Passphrases must match.</p>}
          </>}
          <div className="flex flex-wrap gap-2">
            <Button type="submit" disabled={!!busy || unavailable || !canSave}>Save backup</Button>
            <Button type="button" variant="outline" disabled={!!busy || unavailable} onClick={() => run(true)}>Choose archive to restore</Button>
          </div>
          <label className="flex items-center gap-2 text-sm font-medium">
            <Checkbox checked={repairMissingAudio} disabled={!!busy || unavailable}
              onCheckedChange={checked => setRepairMissingAudio(checked === true)} />
            Repair missing audio for existing meetings
          </label>
        </form>
      )}
      {busy && <p role="status" className="text-sm">{busy} Keep ClawScribe open until this finishes.</p>}
      {unavailable && <p role="status" className="text-sm">Finish the active recording before using backup or restore.</p>}
      {visible && <ScheduledBackups disabled={!!busy || unavailable} />}
    </DialogContent></Dialog>
  </>;
}
