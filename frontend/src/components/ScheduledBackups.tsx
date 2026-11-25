'use client';

import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Button } from '@/components/ui/button';
import { Switch } from '@/components/ui/switch';
import { formatArchiveBytes } from '@/lib/archive-review';

interface ScheduleStatus {
  enabled: boolean;
  exclude_audio: boolean;
  running: boolean;
  destination: string | null;
  last_success: number | null;
  last_backup_audio_excluded: boolean;
  last_error: string | null;
  incomplete_meetings: number;
  skipped_template_items: number;
  omitted_saved_live_answers: number;
}

interface ArchiveStorage {
  saved_audio_bytes: number;
  database_bytes: number;
  reference_bytes: number;
  unavailable_folders: number;
  partial: boolean;
}

export function ScheduledBackups({ disabled }: { disabled: boolean }) {
  const [status, setStatus] = useState<ScheduleStatus | null>(null);
  const [excludeBeforeChoosingFolder, setExcludeBeforeChoosingFolder] = useState(false);
  const [storage, setStorage] = useState<ArchiveStorage | 'unavailable' | null>(null);
  const [error, setError] = useState('');
  const [saving, setSaving] = useState(false);
  const pending = useRef(false);
  const statusVersion = useRef(0);
  const switchId = useId();
  const excludeId = useId();

  const refresh = useCallback(async () => {
    const request = ++statusVersion.current;
    try {
      const next = await invoke<ScheduleStatus>('scheduled_backup_status');
      if (request === statusVersion.current) {
        setStatus(next);
        setError('');
      }
    } catch (failure) {
      if (request === statusVersion.current) setError(String(failure));
    }
  }, []);
  useEffect(() => {
    let active = true;
    const refreshIfActive = () => { if (active) void refresh(); };
    const subscription = listen('backup-schedule-changed', refreshIfActive);
    void subscription.then(refreshIfActive, refreshIfActive);
    return () => {
      active = false;
      statusVersion.current += 1;
      void subscription.then(unlisten => unlisten(), () => {});
    };
  }, [refresh]);

  useEffect(() => {
    if (disabled) return;
    let active = true;
    setStorage(null);
    void invoke<ArchiveStorage>('measure_archive_storage')
      .then(value => { if (active) setStorage(value); })
      .catch(() => { if (active) setStorage('unavailable'); });
    return () => { active = false; };
  }, [disabled]);

  const change = async (options?: { enabled: boolean } | { excludeAudio: boolean }) => {
    if (pending.current) return;
    pending.current = true;
    setSaving(true);
    setError('');
    try {
      if (!options) {
        await invoke('choose_scheduled_backup_folder', {
          excludeAudio: status?.destination ? undefined : excludeBeforeChoosingFolder,
        });
      } else {
        await invoke('set_scheduled_backups', options);
      }
      await refresh();
    } catch (failure) {
      setError(String(failure));
    } finally {
      pending.current = false;
      setSaving(false);
    }
  };

  const unavailable = disabled || saving || !!status?.running;
  const excludeAudio = status?.destination ? status.exclude_audio : excludeBeforeChoosingFolder;
  const estimate = storage && storage !== 'unavailable'
    ? storage.database_bytes + storage.reference_bytes + (excludeAudio ? 0 : storage.saved_audio_bytes)
    : null;
  return (
    <section aria-label="Scheduled backups" className="space-y-3 border-t border-border pt-4">
      <div>
        <h3 className="text-sm font-medium">Scheduled backups</h3>
        <p className="text-sm text-muted-foreground">Save a backup each day while ClawScribe is open. Keep the three newest scheduled copies.</p>
      </div>
      {!status && !error && <p role="status" className="text-sm">Loading scheduled backup settings…</p>}
      {status && <>
        <div className="flex items-center gap-2">
          <Switch id={excludeId} aria-describedby={`${excludeId}-retention`} checked={excludeAudio} disabled={unavailable} onCheckedChange={checked => {
            if (status.destination) void change({ excludeAudio: checked });
            else setExcludeBeforeChoosingFolder(checked);
          }} />
          <label htmlFor={excludeId} className="text-sm">Exclude recording audio</label>
        </div>
        <p id={`${excludeId}-retention`} className="text-sm text-muted-foreground">Older scheduled copies with audio rotate out after three new backups with audio excluded.</p>
        <p role="status" className="text-sm text-muted-foreground">
          {disabled ? 'Size estimate available when the library is idle.'
            : estimate !== null ? `Estimated size per backup is ${formatArchiveBytes(estimate)}.`
            : storage === 'unavailable' ? 'Size estimate unavailable.' : 'Estimating backup size…'}
          {!disabled && storage && storage !== 'unavailable' && (storage.partial || storage.unavailable_folders > 0)
            ? ' Some library files could not be measured.' : ''}
        </p>
      </>}
      {status?.destination && <>
        <p className="break-all text-sm text-muted-foreground">{status.destination}</p>
        <div className="flex items-center gap-2">
          <Switch id={switchId} checked={status.enabled} disabled={unavailable} onCheckedChange={enabled => void change({ enabled })} />
          <label htmlFor={switchId} className="text-sm">Daily backups</label>
        </div>
        <p role="status" className="text-sm text-muted-foreground">
          {status.running ? 'Saving a scheduled backup…' : status.last_success === null
            ? (status.enabled ? 'Waiting for the first backup.' : 'No scheduled backups saved yet.')
            : `Last saved ${new Date(status.last_success * 1000).toLocaleString()}.`}
          {!status.running && status.last_backup_audio_excluded && ' The last backup does not include recording audio.'}
          {!status.running && status.incomplete_meetings > 0 && ` Incomplete audio in ${status.incomplete_meetings} meetings.`}
          {!status.running && status.skipped_template_items > 0 && ` Skipped ${status.skipped_template_items} template ${status.skipped_template_items === 1 ? 'item' : 'items'}.`}
          {!status.running && status.omitted_saved_live_answers > 0 && ` Excluded ${status.omitted_saved_live_answers} saved live ${status.omitted_saved_live_answers === 1 ? 'answer' : 'answers'}.`}
        </p>
      </>}
      {(status || error) && <Button variant="outline" size="sm" disabled={unavailable} onClick={() => void change()}>
        {status?.destination ? 'Change backup folder' : 'Choose folder and enable'}
      </Button>}
      {(error || status?.last_error) && <p role="alert" className="text-sm text-destructive">{error || status?.last_error}</p>}
    </section>
  );
}
