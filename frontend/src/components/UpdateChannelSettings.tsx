'use client';

import { Loader2, RefreshCw } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from './ui/button';
import { Switch } from './ui/switch';
import { useUpdateCheckContext } from './UpdateCheckProvider';
import { updateErrorMessage } from '@/services/updateService';

export function UpdateChannelSettings({ id }: { id: string }) {
  const { ready, channel, isChecking, isInstalling, isSavingChannel, setChannel, checkForUpdates, updateInfo, error } = useUpdateCheckContext();
  const busy = isInstalling || isSavingChannel;
  const change = async (enabled: boolean) => {
    try { await setChannel(enabled ? 'preview' : 'stable'); }
    catch (error) { toast.error(updateErrorMessage(error)); }
  };
  return (
    <div className="space-y-3 text-left">
      <div className="flex items-start justify-between gap-5">
        <div className="min-w-0">
          <label htmlFor={id} className="text-sm font-medium">Include prereleases</label>
          <p id={`${id}-description`} className="mt-1 text-sm text-muted-foreground">Get early access to upcoming features. Previews may be less stable. You choose when to install.</p>
        </div>
        <Switch id={id} checked={channel === 'preview'} onCheckedChange={enabled => void change(enabled)} disabled={!ready || busy} aria-describedby={`${id}-description`} className="mt-1 shrink-0" />
      </div>
      <p className="text-xs text-muted-foreground">{channel === 'preview' ? 'Stable releases and prereleases. Turning this off waits for a newer stable version; it never downgrades.' : 'Stable releases only (default).'}</p>
      <div className="flex flex-wrap items-center gap-3">
        <Button size="sm" variant="outline" disabled={busy || isChecking} onClick={() => void checkForUpdates(true)}>
          {isChecking || isSavingChannel ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : <RefreshCw className="mr-2 h-4 w-4" />}
          {isChecking ? 'Checking…' : 'Check for updates'}
        </Button>
        {updateInfo?.available && <span className="text-sm text-primary">{updateInfo.prerelease ? 'Prerelease' : 'Stable'} {updateInfo.version} available</span>}
      </div>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
    </div>
  );
}
