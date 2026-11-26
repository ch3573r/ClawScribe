import { useState } from 'react';
import { useRouter } from 'next/navigation';
import { AudioLines, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Progress } from '@/components/ui/progress';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { useBackgroundImportQueue, useImportDialog } from '@/contexts/ImportDialogContext';
import { ImportSpeakerDetectionButton } from '@/components/ImportAudio/ImportSpeakerDetectionButton';

export function SidebarImportStatus({ collapsed }: { collapsed: boolean }) {
  const { items, error } = useBackgroundImportQueue();
  const { openImportDialog } = useImportDialog();
  const router = useRouter();
  const [dismissed, setDismissed] = useState<string | null>(null);
  const active = items.find(item => item.status === 'running' || item.status === 'cancelling')
    ?? items.find(item => item.status === 'queued');
  const recovered = items.findLast(item => item.status === 'saved' && item.result);
  const completed = recovered?.id === dismissed ? undefined : recovered;
  const result = completed?.result;
  const message = active?.status === 'cancelling' ? 'Cancelling import…'
    : active?.status === 'queued' ? 'Waiting to import'
    : active?.progress?.message ?? 'Importing media…';
  const progress = active?.progress?.progress_percentage;
  const openMeeting = (id: string) => router.push(`/meeting-details?id=${encodeURIComponent(id)}`);

  if (!active && !result) return null;
  if (collapsed) {
    const label = active ? progress === undefined ? message : `${message} ${Math.round(progress)}%` : 'Import saved';
    return <div className="flex flex-col items-center pb-3">
      <Tooltip>
        <TooltipTrigger asChild>
          <Button variant="ghost" size="icon" aria-label={label} onClick={() => openImportDialog()}>
            <AudioLines aria-hidden="true" />
          </Button>
        </TooltipTrigger>
        <TooltipContent side="right">{label}</TooltipContent>
      </Tooltip>
      {!active && result && <ImportSpeakerDetectionButton result={result} compact />}
    </div>;
  }
  return <section aria-label="Background imports" className="shrink-0 space-y-2 border-t border-sidebar-border p-3">
    {active && <div className="space-y-1.5">
      <p className="truncate text-sm font-medium text-sidebar-foreground" title={active.title}>{active.title}</p>
      <p className="text-xs text-muted-foreground" role="status">{error ? 'Could not refresh import progress.' : message}</p>
      {active.status !== 'queued' && <Progress value={progress ?? null} aria-label={`Import progress for ${active.title}`} />}
    </div>}
    {completed && result && <div>
      <div className="flex items-center justify-between gap-2">
        <p className="truncate text-xs text-muted-foreground" title={completed.title}>Saved · {completed.title}</p>
        <Button variant="ghost" size="icon" className="size-7 shrink-0" aria-label="Dismiss import result" onClick={() => setDismissed(completed.id)}>
          <X aria-hidden="true" />
        </Button>
      </div>
      <div className="flex flex-wrap gap-1">
        <Button size="sm" variant="ghost" onClick={() => openMeeting(result.meeting_id)}>Open meeting</Button>
        <ImportSpeakerDetectionButton result={result} />
      </div>
    </div>}
  </section>;
}
