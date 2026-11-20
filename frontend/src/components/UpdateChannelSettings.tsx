'use client';

import { useEffect, useRef, useState } from 'react';
import { Loader2, RefreshCw } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from './ui/button';
import { Switch } from './ui/switch';
import { Meta, SettingsFooter, SettingsRow, StatusText, type StatusTone } from './ui/settings-layout';
import { NoteRow } from './settings/settings-parts';
import { useUpdateCheckContext } from './UpdateCheckProvider';
import { updateErrorMessage, type UpdateSnapshot } from '@/services/updateService';

/** The one-line state of the update check, shown with a status dot. */
export function describeUpdateStatus(snapshot: Pick<UpdateSnapshot, 'ready' | 'currentVersion' | 'isChecking' | 'isInstalling' | 'isSavingChannel' | 'updateInfo'>): { tone: StatusTone; text: string } {
  const { ready, currentVersion, isChecking, isInstalling, isSavingChannel, updateInfo } = snapshot;
  if (isInstalling) return { tone: 'active', text: 'Installing the update…' };
  if (isSavingChannel) return { tone: 'neutral', text: 'Saving the update channel…' };
  if (isChecking) return { tone: 'neutral', text: 'Checking for updates…' };
  if (updateInfo?.available) return { tone: 'active', text: `${updateInfo.prerelease ? 'Prerelease' : 'Stable'} ${updateInfo.version} available` };
  if (updateInfo) return { tone: 'ok', text: currentVersion ? `${currentVersion} is up to date` : 'ClawScribe is up to date' };
  if (!ready) return { tone: 'neutral', text: 'Loading update settings…' };
  return { tone: 'neutral', text: currentVersion ? `${currentVersion} · not checked yet` : 'Not checked yet' };
}

/** "Checked today 10:42" or "Checked 12 Nov 10:42". */
export function checkedLabel(at: Date, now = new Date()): string {
  const time = at.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  const sameDay = at.getFullYear() === now.getFullYear() && at.getMonth() === now.getMonth() && at.getDate() === now.getDate();
  if (sameDay) return `Checked today ${time}`;
  return `Checked ${at.toLocaleDateString([], { day: 'numeric', month: 'short' })} ${time}`;
}

/**
 * The prerelease switch and the update status line. Rendered inside the
 * Updates card, below the "check at startup" row.
 */
export function UpdateChannelSettings({ id }: { id: string }) {
  const snapshot = useUpdateCheckContext();
  const { ready, channel, isChecking, isInstalling, isSavingChannel, setChannel, checkForUpdates, updateInfo, error } = snapshot;
  const busy = isInstalling || isSavingChannel;

  // The service does not expose when it last succeeded. Note a result only when
  // this page sees a new one arrive, so a time is never invented.
  const [checkedAt, setCheckedAt] = useState<Date | null>(null);
  const seenInfo = useRef(updateInfo);
  useEffect(() => {
    if (updateInfo === seenInfo.current) return;
    seenInfo.current = updateInfo;
    setCheckedAt(updateInfo ? new Date() : null);
  }, [updateInfo]);

  const change = async (enabled: boolean) => {
    try { await setChannel(enabled ? 'preview' : 'stable'); }
    catch (error) { toast.error(updateErrorMessage(error)); }
  };
  const status = describeUpdateStatus(snapshot);

  return (
    <>
      <SettingsRow
        title="Include prereleases"
        htmlFor={id}
        description={
          <span id={`${id}-description`}>
            {channel === 'preview'
              ? 'Stable releases and prereleases. Early builds can be less stable. You choose when to install; turning this off waits for a newer stable version and never downgrades'
              : 'Stable releases only (default). Turn on to get early builds, which can be less stable. You choose when to install'}
          </span>
        }
      >
        <Switch id={id} checked={channel === 'preview'} onCheckedChange={enabled => void change(enabled)} disabled={!ready || busy} aria-describedby={`${id}-description`} />
      </SettingsRow>
      {error && <NoteRow tone="error">{error}</NoteRow>}
      <SettingsFooter
        actions={
          <Button type="button" size="sm" variant="outline" disabled={busy || isChecking} onClick={() => void checkForUpdates(true)}>
            {isChecking || isSavingChannel ? <Loader2 aria-hidden="true" className="animate-spin" /> : <RefreshCw aria-hidden="true" />}
            {isChecking ? 'Checking…' : 'Check now'}
          </Button>
        }
      >
        <span role="status"><StatusText tone={status.tone}>{status.text}</StatusText></span>
        {checkedAt && <Meta>{checkedLabel(checkedAt)}</Meta>}
      </SettingsFooter>
    </>
  );
}
