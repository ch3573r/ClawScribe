'use client';

import { useEffect, useId, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Button } from '@/components/ui/button';
import { Switch } from '@/components/ui/switch';

interface ScheduleStatus {
  enabled: boolean;
  running: boolean;
  destination: string | null;
  last_success: number | null;
  last_error: string | null;
  incomplete_meetings: number;
}

export function ScheduledBackups({ disabled }: { disabled: boolean }) {
  const [status, setStatus] = useState<ScheduleStatus | null>(null);
  const [error, setError] = useState('');
  const [saving, setSaving] = useState(false);
  const pending = useRef(false);
  const switchId = useId();

  useEffect(() => {
    let active = true;
    let latest = 0;
    const refresh = async () => {
      const request = ++latest;
      try {
        const next = await invoke<ScheduleStatus>('scheduled_backup_status');
        if (active && request === latest) {
          setStatus(next);
          setError('');
        }
      } catch (failure) {
        if (active && request === latest) setError(String(failure));
      }
    };
    const subscription = listen('backup-schedule-changed', () => void refresh());
    void subscription.then(refresh, refresh);
    return () => {
      active = false;
      void subscription.then(unlisten => unlisten(), () => {});
    };
  }, []);

  const change = async (enabled?: boolean) => {
    if (pending.current) return;
    pending.current = true;
    setSaving(true);
    setError('');
    try {
      if (enabled === undefined) {
        await invoke('choose_scheduled_backup_folder');
      } else {
        await invoke('set_scheduled_backups', { enabled });
      }
      setStatus(await invoke<ScheduleStatus>('scheduled_backup_status'));
    } catch (failure) {
      setError(String(failure));
    } finally {
      pending.current = false;
      setSaving(false);
    }
  };

  const unavailable = disabled || saving || !!status?.running;
  return (
    <section aria-label="Scheduled backups" className="space-y-3 border-t border-border pt-4">
      <div>
        <h3 className="text-sm font-medium">Scheduled backups</h3>
        <p className="text-sm text-muted-foreground">Save a backup each day while ClawScribe is open. Keep the three newest scheduled copies.</p>
      </div>
      {!status && !error && <p role="status" className="text-sm">Loading scheduled backup settings…</p>}
      {status?.destination && <>
        <p className="break-all text-sm text-muted-foreground">{status.destination}</p>
        <div className="flex items-center gap-2">
          <Switch id={switchId} checked={status.enabled} disabled={unavailable} onCheckedChange={change} />
          <label htmlFor={switchId} className="text-sm">Daily backups</label>
        </div>
        <p role="status" className="text-sm text-muted-foreground">
          {status.running ? 'Saving a scheduled backup…' : status.last_success === null
            ? (status.enabled ? 'Waiting for the first backup.' : 'No scheduled backups saved yet.')
            : `Last saved ${new Date(status.last_success * 1000).toLocaleString()}.`}
          {!status.running && status.incomplete_meetings > 0 && ` Incomplete audio in ${status.incomplete_meetings} meetings.`}
        </p>
      </>}
      {(status || error) && <Button variant="outline" size="sm" disabled={unavailable} onClick={() => void change()}>
        {status?.destination ? 'Change backup folder' : 'Choose folder and enable'}
      </Button>}
      {(error || status?.last_error) && <p role="alert" className="text-sm text-destructive">{error || status?.last_error}</p>}
    </section>
  );
}
