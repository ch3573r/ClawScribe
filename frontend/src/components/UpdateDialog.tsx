import React, { useEffect, useRef, useState } from 'react';
import { AlertCircle, CheckCircle2, Download, Loader2 } from 'lucide-react';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from './ui/dialog';
import { Button } from './ui/button';
import { useUpdateCheckContext } from './UpdateCheckProvider';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { updateService, type UpdateProgress } from '@/services/updateService';

export function UpdateDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const { updateInfo, channel, currentVersion, isChecking, isInstalling, isSavingChannel, error, checkForUpdates } = useUpdateCheckContext();
  const recording = useRecordingState();
  const canInstall = !(recording.isRecording || recording.isStarting || recording.isStopping || recording.isProcessing || recording.isSaving);
  const canInstallRef = useRef(canInstall);
  canInstallRef.current = canInstall;
  const [progress, setProgress] = useState<UpdateProgress | null>(null);
  const busy = isChecking || isInstalling || isSavingChannel;
  useEffect(() => { if (!isInstalling) setProgress(null); }, [open, updateInfo, isInstalling]);

  const install = async () => {
    if (!updateInfo || !canInstallRef.current) return;
    try {
      await updateService.downloadAndInstall(updateInfo, setProgress, () => canInstallRef.current);
    } catch { /* The shared service keeps the actionable error visible. */ }
  };
  const title = isInstalling ? (progress?.phase === 'installing' ? 'Installing update' : 'Downloading update')
    : isChecking ? 'Checking for updates' : error ? 'Update could not be completed'
    : updateInfo?.available ? 'Update available' : 'ClawScribe updates';

  return (
    <Dialog open={open} onOpenChange={value => { if (!isInstalling) onOpenChange(value); }}>
      <DialogContent className="sm:max-w-[500px]" onEscapeKeyDown={event => { if (isInstalling) event.preventDefault(); }} onInteractOutside={event => { if (isInstalling) event.preventDefault(); }}>
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            {busy ? <Loader2 className="h-5 w-5 animate-spin text-primary" /> : error ? <AlertCircle className="h-5 w-5 text-destructive" /> : <Download className="h-5 w-5 text-primary" />}
            {title}
          </DialogTitle>
          <DialogDescription>{isInstalling ? 'ClawScribe will restart after installation.' : channel === 'preview' ? 'Checking stable releases and prereleases.' : 'Checking stable releases only.'}</DialogDescription>
        </DialogHeader>
        <div className="space-y-4 py-2" aria-live="polite">
          {currentVersion && <p className="text-sm text-muted-foreground">Installed version: {currentVersion}</p>}
          {!busy && !error && updateInfo && !updateInfo.available && <p className="flex items-start gap-2 text-sm"><CheckCircle2 className="h-4 w-4 shrink-0 text-primary" />No newer {channel === 'preview' ? 'stable release or prerelease' : 'stable release'} is available.</p>}
          {!busy && updateInfo?.available && (
            <>
              <div className="flex items-center justify-between gap-3"><span className="text-lg font-semibold">ClawScribe {updateInfo.version}</span><span className="rounded-full bg-primary/10 px-2.5 py-1 text-xs font-medium text-primary">{updateInfo.prerelease ? 'Prerelease' : 'Stable'}</span></div>
              {updateInfo.prerelease && <p className="text-sm text-muted-foreground">This preview may contain unfinished features. Turning off prereleases later waits for a newer stable version; it does not downgrade your installation.</p>}
              {updateInfo.body && <div className="max-h-48 overflow-y-auto rounded-md bg-muted p-3 text-sm whitespace-pre-wrap break-words">{updateInfo.body}</div>}
            </>
          )}
          {isInstalling && progress && (
            <div className="space-y-2">
              <div role="progressbar" aria-label="Update download" aria-valuemin={0} aria-valuemax={100} aria-valuenow={progress.total ? progress.percentage : undefined} className="h-2 overflow-hidden rounded-full bg-secondary"><div className={`h-full bg-primary transition-[width] ${progress.total ? '' : 'animate-pulse'}`} style={{ width: progress.total ? `${progress.percentage}%` : '100%' }} /></div>
              <p className="text-sm text-muted-foreground">{progress.phase === 'installing' ? 'Installer starting…' : `${(progress.downloaded / 1024 / 1024).toFixed(1)} MB downloaded${progress.total ? ` (${progress.percentage}%)` : ''}`}</p>
            </div>
          )}
          {error && <p role="alert" className="rounded-md border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">{error}</p>}
          {!canInstall && updateInfo?.available && <p className="text-sm text-muted-foreground">Finish recording and saving your meeting before installing.</p>}
        </div>
        <DialogFooter>
          <Button variant="outline" disabled={isInstalling} onClick={() => onOpenChange(false)}>{updateInfo?.available ? 'Later' : 'Close'}</Button>
          {error || !updateInfo ? <Button disabled={busy} onClick={() => void checkForUpdates(true)}>Try again</Button> : updateInfo.available && <Button disabled={busy || !canInstall} onClick={() => void install()}><Download className="mr-2 h-4 w-4" />Download &amp; install</Button>}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
