'use client';
import { Switch } from '@/components/ui/switch';
import { PageSection } from '@/components/ui/page-section';
import { useLiveAssistance } from '@/hooks/useLiveAssistance';
import { knowledgeProviderLabel } from '@/lib/knowledge-documents';

export function TranscriptSharingControl({ state, actions }: ReturnType<typeof useLiveAssistance>) {
  return <div className="flex items-start justify-between gap-4">
    <div><label htmlFor="live-transcript-sharing" className="text-sm font-medium">Share live transcript with the provider</label>
      <p className="mt-1 text-xs text-muted-foreground">Off by default. Remembered for future recordings. Only a manual action sends finalized text to {knowledgeProviderLabel(state.provider)}.</p></div>
    <Switch id="live-transcript-sharing" checked={state.transcriptSharing} disabled={!state.sharingReady || state.sharingBusy}
      onCheckedChange={enabled => void actions.setTranscriptSharing(enabled)} />
  </div>;
}

/** Settings and the panel use the same backend-owned remembered permission. */
export function LiveSharingSetting() {
  const live = useLiveAssistance();
  return <PageSection title="Live assistance" description="Manual questions during recording use your summary provider.">
    <div className="space-y-3 p-5">{TranscriptSharingControl(live)}
      {!live.state.sharingReady && <p role="status" className="text-sm text-muted-foreground">Loading live sharing…</p>}
      {live.state.sharingBusy && <p role="status" className="text-sm text-muted-foreground">Saving live sharing…</p>}
      {live.state.error && <p role="alert" className="text-sm text-destructive">{live.state.error}</p>}
      <p className="text-xs text-muted-foreground">Reference-document sharing is separate and resets for each recording. Built-in AI is available after recording.</p>
    </div>
  </PageSection>;
}
