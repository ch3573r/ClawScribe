'use client';

import { useEffect, useRef, useState } from 'react';
import { useRouter } from 'next/navigation';
import { ChevronDown, ChevronUp } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Switch } from '@/components/ui/switch';
import { Checkbox } from '@/components/ui/checkbox';
import { Textarea } from '@/components/ui/textarea';
import { MultiSelect } from '@/components/ui/multi-select';
import { PageSection } from '@/components/ui/page-section';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { CitedAnswer } from '@/components/Knowledge/KnowledgeChat';
import { TranscriptSharingControl } from '@/components/LiveTranscriptSharing';
import { useLiveAssistance } from '@/hooks/useLiveAssistance';
import { useRecordingState } from '@/contexts/RecordingStateContext';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { knowledgeService } from '@/services/knowledgeService';
import { documentAnchor, knowledgeProviderLabel } from '@/lib/knowledge-documents';
import type { EvidenceDisplay, EvidenceRef, ResolvedEvidence } from '@/types/knowledge';

function time(seconds: number) {
  const total = Math.max(0, Math.floor(seconds));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, '0')}`;
}
/** The existing transcript view owns scrolling; this panel adds no scroll lane. */
export function LiveAssistancePanel() {
  const live = useLiveAssistance();
  const { state, actions } = live;
  const recording = useRecordingState();
  const { meetings, currentMeeting } = useSidebar();
  const router = useRouter();
  const [expanded, setExpanded] = useState(false);
  const [question, setQuestion] = useState('');
  const [referencesOpen, setReferencesOpen] = useState(false);
  const [preview, setPreview] = useState<{ sessionId: string; metadata: EvidenceDisplay; resolved: ResolvedEvidence | null } | null>(null);
  const [previewBusy, setPreviewBusy] = useState(false);
  const [previewError, setPreviewError] = useState('');
  const previewRevision = useRef(0);
  useEffect(() => {
    previewRevision.current++;
    setPreview(null); setPreviewBusy(false); setPreviewError(''); setQuestion('');
    return () => { previewRevision.current++; };
  }, [state.sessionId]);
  const audioOnly = recording.sessionMode === 'audio_only' || state.snapshot?.transcription_available === false;
  const builtin = state.provider === 'builtin-ai';
  const statusError = recording.verificationError ?? state.snapshotError;
  const ready = !!state.sessionId && !!state.snapshot?.transcription_available && state.snapshot.finalized_through_seconds > 0 &&
    !!state.provider && !!state.model && !builtin && state.sharingReady && state.transcriptSharing && !state.sharingBusy &&
    !state.pending && !state.documentSharingBusy && !state.documentsLoading && !statusError &&
    !(state.referenceError && state.meetingIds.length);
  const submit = (text: string) => { if (ready && text.trim()) void actions.ask(text); };
  async function inspect(reference: EvidenceRef, metadata: EvidenceDisplay, contextReference?: EvidenceRef) {
    const sessionId = actions.getSnapshot().sessionId;
    if (!sessionId) return;
    const revision = ++previewRevision.current;
    const current = () => revision === previewRevision.current && actions.getSnapshot().sessionId === sessionId;
    setPreview({ sessionId, metadata, resolved: null }); setPreviewBusy(true); setPreviewError('');
    try {
      if (contextReference) {
        const context = await knowledgeService.resolve(contextReference);
        if (!current()) return;
        if (context.status !== 'current') { setPreview({ sessionId, metadata, resolved: { status: context.status, passage: null, navigation: null } }); return; }
      }
      const resolved = await knowledgeService.resolve(reference);
      if (current()) setPreview({ sessionId, metadata, resolved });
    } catch { if (current()) setPreviewError('Could not verify this source. Retry the source or ask again.'); }
    finally { if (current()) setPreviewBusy(false); }
  }
  const visiblePreview = preview?.sessionId === state.sessionId ? preview : null;
  const source = visiblePreview?.resolved;
  const locator = source?.passage?.evidence.locator;
  const selectedNames = meetings.filter(meeting => state.meetingIds.includes(meeting.id)).map(meeting => meeting.title);
  const selectedDocuments = state.documents.filter(row => state.documentIds.includes(row.attachment.id));
  return <PageSection className="mb-5" aria-label="Live assistance">
    <div className="space-y-2 p-4">
      <div className="flex items-center justify-between gap-3">
        <Button variant="ghost" size="sm" className="h-auto justify-start px-0 text-left" aria-expanded={expanded} aria-controls="live-assistance-body" onClick={() => setExpanded(value => !value)}>
          {expanded ? <ChevronUp aria-hidden="true" /> : <ChevronDown aria-hidden="true" />} Live assistance
        </Button>
        <span className="text-xs text-muted-foreground">Manual · Preview</span>
      </div>
      <p role="status" className="text-xs text-muted-foreground">{state.snapshot
        ? `Finalized through ${time(state.snapshot.finalized_through_seconds)}${state.snapshot.transcription_incomplete ? ' · Transcript incomplete' : ''}`
        : state.sessionId ? 'Reading finalized transcript status…' : 'Available during a recording with live transcription.'}</p>
      <p className="text-xs text-muted-foreground">Uses the last 10 minutes of finalized speech. Recent speech may still be behind.</p>
      {statusError && <div className="flex flex-wrap items-center gap-2"><p role="alert" className="text-xs text-destructive">{statusError}</p>
        <Button variant="outline" size="sm" onClick={() => { recording.retryVerification(); void actions.refresh(); }}>Retry status</Button></div>}
    </div>
    {expanded && <div id="live-assistance-body" className="space-y-4 border-t border-border p-4">
      <div className="flex flex-wrap items-center gap-2 text-sm"><span>Provider: {knowledgeProviderLabel(state.provider)}</span>
        {state.model && <Tooltip><TooltipTrigger asChild><Button variant="ghost" size="sm">Selected model</Button></TooltipTrigger><TooltipContent>{state.model}</TooltipContent></Tooltip>}
      </div>
      {TranscriptSharingControl(live)}
      {state.sharingBusy && <p role="status" className="text-xs text-muted-foreground">Saving live sharing…</p>}
      {state.sessionId && state.snapshot?.transcription_available && state.snapshot.finalized_through_seconds <= 0 &&
        <p role="status" className="text-sm text-muted-foreground">Waiting for finalized speech. Ask when transcription has caught up.</p>}
      {state.provider === 'ollama' && <p className="text-xs text-muted-foreground">Ollama runs separately on this PC and can compete with transcription for memory and compute. Cancel assistance if recording falls behind.</p>}
      {audioOnly ? <p role="status" className="text-sm text-muted-foreground">Audio-only mode has no live transcript. Stop, transcribe the saved meeting, then use Chat with this meeting.</p>
        : builtin ? <p role="status" className="text-sm text-muted-foreground">Built-in AI is unavailable during recording. After recording, open the saved meeting and use Chat with this meeting.</p>
        : !state.sessionId ? <p role="status" className="text-sm text-muted-foreground">Live messages clear when recording stops. Open the saved meeting to continue with Chat with this meeting.</p>
        : !state.provider || !state.model ? <p role="status" className="text-sm text-muted-foreground">Choose a summary provider and model in Settings.</p> : null}
      {recording.status === 'completed' && currentMeeting && <Button variant="outline" onClick={() => router.push(`/meeting-details?id=${encodeURIComponent(currentMeeting.id)}`)}>Open saved meeting</Button>}
      <div className="space-y-2">
        <Button variant="outline" size="sm" aria-expanded={referencesOpen} aria-controls="live-reference-selection" disabled={!state.sessionId || state.pending} onClick={() => setReferencesOpen(value => !value)}>Selected references</Button>
        <p className="break-words text-xs text-muted-foreground">Current recording{selectedNames.length ? ` + ${selectedNames.join(', ')}` : ' only'}
          {selectedDocuments.length ? ` · ${selectedDocuments.map(row => row.attachment.display_name).join(', ')}${state.documentSharing ? '' : ' (document sharing off)'}` : ''}</p>
        {referencesOpen && <div id="live-reference-selection" className="space-y-3">
          <label htmlFor="live-reference-meetings" className="block text-sm font-medium">Saved meetings to include</label>
          <MultiSelect id="live-reference-meetings" value={state.meetingIds} disabled={state.pending || state.documentsLoading}
            className="w-full rounded-md border border-input bg-background p-2 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            onChange={event => void actions.setReferences(Array.from(event.target.selectedOptions, option => option.value), [])}>
            {meetings.map(meeting => <option key={meeting.id} value={meeting.id}>{meeting.title}</option>)}
          </MultiSelect>
          {!meetings.length && <p className="text-xs text-muted-foreground">No saved meetings yet. Use the current recording alone.</p>}
          {state.documentsLoading && <p role="status" className="text-xs text-muted-foreground">Loading references…</p>}
          {state.documents.map(row => <label key={row.attachment.id} className="flex items-start gap-2 text-sm">
            <Checkbox checked={state.documentIds.includes(row.attachment.id)} disabled={state.pending || state.documentsLoading || row.attachment.extraction_status !== 'ready'}
              onCheckedChange={checked => void actions.setReferences(state.meetingIds, checked === true ? [...state.documentIds, row.attachment.id] : state.documentIds.filter(id => id !== row.attachment.id))} />
            <span className="break-words">{row.attachment.display_name}{row.attachment.extraction_status !== 'ready' ? ' · Text unavailable' : ''}</span>
          </label>)}
          {!!state.meetingIds.length && !state.documentsLoading && !state.documents.length && !state.referenceError && <p className="text-xs text-muted-foreground">No reference documents attached to these meetings.</p>}
        </div>}
        <div className="flex items-start justify-between gap-4"><div><label htmlFor="live-document-sharing" className="text-sm font-medium">Share selected documents for this recording</label>
          <p className="mt-1 text-xs text-muted-foreground">Independent of transcript sharing. Resets when recording stops.</p></div>
          <Switch id="live-document-sharing" checked={state.documentSharing} disabled={!state.sessionId || state.documentSharingBusy}
            onCheckedChange={enabled => void actions.setDocumentSharing(enabled)} /></div>
        {state.documentSharingBusy && <p role="status" className="text-xs text-muted-foreground">Updating reference sharing…</p>}
        {state.referenceError && <div className="flex flex-wrap items-center gap-2"><p role="alert" className="text-xs text-destructive">{state.referenceError}</p>
          {!!state.meetingIds.length && <Button variant="outline" size="sm" onClick={() => void actions.setReferences(state.meetingIds, state.documentIds)}>Retry references</Button>}</div>}
      </div>
      <form className="space-y-3" onSubmit={event => { event.preventDefault(); submit(question); }}>
        <label htmlFor="live-question" className="block text-sm font-medium">Question</label>
        <Textarea id="live-question" value={question} rows={2} className="min-h-16 resize-none" placeholder="What remains open?" disabled={audioOnly || builtin || !state.sessionId}
          onChange={event => setQuestion(event.target.value)} onKeyDown={event => {
            if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent?.isComposing) { event.preventDefault(); submit(question); }
          }} />
        <div className="flex flex-wrap items-center gap-2"><Button type="submit" disabled={!ready || !question.trim()}>Ask</Button>
          <Button type="button" variant="outline" disabled={!ready} onClick={() => submit('Summarize the finalized conversation so far. Identify decisions and uncertainty, and acknowledge missing or delayed speech.')}>Summarize so far</Button>
          <Button type="button" variant="outline" disabled={!ready} onClick={() => submit('List open questions from the finalized conversation. Do not invent answers, owners or commitments.')}>List open questions</Button>
          {state.pending && <Button type="button" variant="outline" onClick={actions.cancel}>Cancel</Button>}
        </div>
        {!state.transcriptSharing && <p className="text-xs text-muted-foreground">Enable live transcript sharing to ask. No provider action runs automatically.</p>}
        <p className="text-xs text-muted-foreground">Enter asks · Shift+Enter adds a new line. Live messages are temporary and clear on Stop.</p>
      </form>
      {state.pending && <p role="status" className="text-sm text-muted-foreground">Answering from finalized speech… You can cancel; the request limit is 30 seconds.</p>}
      {state.error && <p role="alert" className="text-sm text-destructive">{state.error}</p>}
      <div className="space-y-3" aria-live="polite">{state.messages.map(({ question: prompt, reply }) => <article key={reply.message_id} className="space-y-2 rounded-md bg-muted p-3">
        <p className="whitespace-pre-wrap break-words text-sm font-medium">{prompt}</p>
        <CitedAnswer reply={reply} state={{ inspect }} />
        <p className="text-xs text-muted-foreground">{knowledgeProviderLabel(reply.provider)} · Finalized through {time(reply.live_context?.finalized_through_seconds ?? 0)}
          {reply.live_context?.transcription_incomplete ? ' · Transcript incomplete' : ''}</p>
      </article>)}</div>
      <Dialog open={!!visiblePreview} onOpenChange={open => { if (!open) { previewRevision.current++; setPreview(null); setPreviewBusy(false); } }}>
        <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-xl"><DialogHeader><DialogTitle>Source passage</DialogTitle>
          <DialogDescription>Verified finalized speech or a selected saved reference. Live sources expire after Stop or when they leave the context window.</DialogDescription></DialogHeader>
          {visiblePreview && <h3 className="break-words font-semibold">{visiblePreview.metadata.title}</h3>}
          {locator?.kind === 'document' && <p className="text-sm">{documentAnchor(locator)}</p>}
          {previewBusy && <p role="status">Verifying source…</p>}
          {previewError && <p role="alert" className="text-sm text-destructive">{previewError}</p>}
          {source?.status === 'current' && source.passage ? <blockquote className="whitespace-pre-wrap break-words border-l-2 border-primary pl-3 text-sm">{source.passage.text}</blockquote>
            : source && <p role="status" className="text-sm text-muted-foreground">This source has expired, changed or become unavailable. Ask again using current evidence.</p>}
        </DialogContent>
      </Dialog>
    </div>}
  </PageSection>;
}
