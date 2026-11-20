'use client';
import { useId } from 'react';
import { Switch } from '@/components/ui/switch';
import { SettingsCard, SettingsFooter, SettingsRow } from '@/components/ui/settings-layout';
import { useLiveAssistance } from '@/hooks/useLiveAssistance';
import { knowledgeProviderLabel } from '@/lib/knowledge-documents';

/**
 * The remembered live-transcript permission. The recording panel renders it as
 * a plain labelled switch; the settings page uses `row` for a settings card.
 */
export function TranscriptSharingControl({ state, actions, row }: ReturnType<typeof useLiveAssistance> & { row?: boolean }) {
  const sharingId = useId();
  const toggle = <Switch id={sharingId} checked={state.transcriptSharing} disabled={!state.sharingReady || state.sharingBusy}
    onCheckedChange={enabled => void actions.setTranscriptSharing(enabled)} />;
  if (row) {
    return <SettingsRow title="Share the live transcript with the provider" htmlFor={sharingId}
      description={`Text is sent to ${knowledgeProviderLabel(state.provider)} only when you ask a question`}>{toggle}</SettingsRow>;
  }
  return <div className="flex items-start justify-between gap-4">
    <div><label htmlFor={sharingId} className="text-sm font-medium">Share live transcript with the provider</label>
      <p className="mt-1 text-xs text-muted-foreground">Off by default. Remembered for future recordings. Only a manual action sends finalized text to {knowledgeProviderLabel(state.provider)}.</p></div>
    {toggle}
  </div>;
}

/** Settings and the panel use the same backend-owned remembered permission. */
export function LiveSharingSetting() {
  const live = useLiveAssistance();
  return <SettingsCard title="Live assistance" description="Ask questions about the meeting while it is being recorded">
    <TranscriptSharingControl {...live} row />
    <SettingsFooter>
      {!live.state.sharingReady && <span role="status">Loading live sharing…</span>}
      {live.state.sharingBusy && <span role="status">Saving live sharing…</span>}
      {live.state.error && <span role="alert" className="text-error-foreground">{live.state.error}</span>}
      {live.state.sharingReady && !live.state.sharingBusy && !live.state.error && (
        <span>Off by default and remembered for future recordings. Reference-document sharing is separate and resets for each recording</span>
      )}
    </SettingsFooter>
  </SettingsCard>;
}
