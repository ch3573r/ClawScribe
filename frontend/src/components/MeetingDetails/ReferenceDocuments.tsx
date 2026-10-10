"use client";
import { useEffect, useRef, useState } from 'react';
import { ChevronDown, MoreHorizontal } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Switch } from '@/components/ui/switch';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu';
import { knowledgeService } from '@/services/knowledgeService';
import { documentAnchor, knowledgeProviderLabel, referenceError } from '@/lib/knowledge-documents';
import { AttachReferenceButton } from './AttachReferenceButton';
import { scopeReady } from '@/lib/knowledge-state';
import type { KnowledgeSearchState } from '@/hooks/useKnowledgeSearch';
import type { DocumentAttachment, DocumentBlock, ScopedDocument } from '@/types/knowledge';

function status(document: DocumentAttachment): string {
  if (document.extraction_status !== 'ready') return document.extraction_status === 'failed' ? 'Extraction failed' : 'Extracting text…';
  const names: Record<string,string> = {ready:'Semantic ready',pending:'Semantic preparing',failed:'Semantic failed',paused:'Semantic paused',disabled:'Semantic off'};
  return `Text ready · ${names[document.indexing_status] ?? 'Semantic status unavailable'}`;
}

export function ReferenceDocuments({state,provider,attachMeetingId,reveal=0}: {
  state: KnowledgeSearchState;provider?:string;attachMeetingId?:string;reveal?:number;
}) {
  const [expanded,setExpanded] = useState(false);
  const [busy,setBusy] = useState(false);
  const [attachmentBusy,setAttachmentBusy] = useState(false);
  const panelBusy = busy || attachmentBusy;
  const [error,setError] = useState('');
  const [preview,setPreview] = useState<{row:ScopedDocument;blocks:DocumentBlock[]}|null>(null);
  const [previewBusy,setPreviewBusy] = useState(false);
  const [previewError,setPreviewError] = useState('');
  const [deleteTarget,setDeleteTarget] = useState<ScopedDocument|null>(null);
  const operationPending = useRef(false);
  const generation = useRef(0);
  const previewGeneration = useRef(0);
  const identity = JSON.stringify({scope:state.scope,meeting:attachMeetingId});
  useEffect(() => {
    generation.current++;
    previewGeneration.current++;
    setPreview(null);setPreviewError('');setPreviewBusy(false);setDeleteTarget(null);setError('');
    return () => {
      generation.current++;
      previewGeneration.current++;
    };
  },[identity]);
  useEffect(() => { if (reveal) setExpanded(true); }, [reveal]);
  async function inspect(row: ScopedDocument) {
    const meetingId = row.meeting_ids[0];
    if (!meetingId) return;
    const current = ++previewGeneration.current;
    setPreview({row,blocks:[]});setPreviewBusy(true);setPreviewError('');
    try {
      const blocks = await knowledgeService.documentBlocks(meetingId,row.attachment.id);
      if (current === previewGeneration.current) setPreview({row,blocks});
    } catch (failure) { if (current === previewGeneration.current) setPreviewError(referenceError(failure)); }
    finally { if (current === previewGeneration.current) setPreviewBusy(false); }
  }
  async function change(row:ScopedDocument,action:'detach'|'delete'|'retry') {
    if (operationPending.current || attachmentBusy) return;
    const meetingId = action === 'detach' ? attachMeetingId : row.meeting_ids[0];
    if (!meetingId) return;
    operationPending.current = true;setBusy(true);setError('');
    const current = generation.current;
    try {
      if (action === 'detach') await knowledgeService.detachDocument(meetingId,row.attachment.id);
      else if (action === 'delete') await knowledgeService.deleteDocument(meetingId,row.attachment.id);
      else await knowledgeService.retryDocument(meetingId,row.attachment.id);
      if (current === generation.current) {
        if (action !== 'retry') state.selectDocument(row.attachment.id,false);
        setDeleteTarget(null);state.reloadDocuments();
      }
    } catch (failure) { if (current === generation.current) setError(referenceError(failure)); }
    finally { operationPending.current = false;setBusy(false); }
  }
  const local = provider === 'builtin-ai';
  const permissionId = `reference-sharing-${state.owner?.id ?? 'new-library'}`;
  const panelId = `reference-documents-${attachMeetingId ?? 'library'}`;
  const ready = scopeReady(state.scope);
  return <section className="space-y-3 border-b border-border pb-4" aria-label="Reference documents">
    <div className="flex flex-wrap items-center gap-2">
      <Button variant="ghost" size="sm" className="mr-auto text-muted-foreground" aria-expanded={expanded} aria-controls={panelId} onClick={() => setExpanded(value => !value)}>
        <ChevronDown className={expanded?'rotate-180':''} />Reference documents{state.documents.length ? ` (${state.documents.length})` : ''}
      </Button>
      {attachMeetingId && <AttachReferenceButton meetingId={attachMeetingId} scopeKey={JSON.stringify(state.scope)} disabled={busy} onBusyChange={setAttachmentBusy} onAttached={() => {setExpanded(true);state.reloadDocuments();}} />}
    </div>
    {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
    {expanded && <div id={panelId} className="space-y-3">
      <p className="text-xs text-muted-foreground">Choose references to include in searches and questions. Files are extracted locally. PDF, DOCX, TXT or Markdown · up to 25 MiB.</p>
      {state.documentsLoading && <p role="status" className="text-sm text-muted-foreground">Loading references…</p>}
      {state.documentsError && <p role="alert" className="text-sm text-destructive">{state.documentsError}</p>}
      {!state.documentsLoading && !state.documents.length && <p className="text-xs text-muted-foreground">{attachMeetingId?'No references attached.':'No references in this scope. Use Attach file above to add one.'}</p>}
      <ul className="space-y-3">{state.documents.map(row => <li key={row.attachment.id} className="flex flex-wrap items-start gap-2">
        <label className="flex min-w-0 flex-1 items-start gap-2 text-sm"><Checkbox className="mt-1" checked={state.selectedDocumentIds.includes(row.attachment.id)} disabled={panelBusy||state.documentsLoading||row.attachment.extraction_status!=='ready'} onCheckedChange={checked => state.selectDocument(row.attachment.id,checked===true)} />
          <span className="min-w-0"><span className="block break-words">{row.attachment.display_name}</span><span className="block text-xs text-muted-foreground">{status(row.attachment)}</span></span>
        </label>
        <Button variant="ghost" size="sm" disabled={panelBusy} onClick={() => void inspect(row)}>Preview</Button>
        {row.attachment.indexing_status==='failed' && <Button variant="outline" size="sm" disabled={panelBusy} onClick={() => void change(row,'retry')}>Retry indexing</Button>}
        <DropdownMenu><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" disabled={panelBusy} aria-label={`Reference actions for ${row.attachment.display_name}`}><MoreHorizontal /></Button></DropdownMenuTrigger><DropdownMenuContent align="end">
          {attachMeetingId && row.meeting_ids.includes(attachMeetingId) && <DropdownMenuItem onSelect={() => void change(row,'detach')}>Detach from this meeting</DropdownMenuItem>}
          <DropdownMenuItem onSelect={() => state.reloadDocuments()}>Refresh status</DropdownMenuItem>
          <DropdownMenuItem className="text-destructive" onSelect={() => setDeleteTarget(row)}>Delete permanently…</DropdownMenuItem>
        </DropdownMenuContent></DropdownMenu>
      </li>)}</ul>
      {state.documents.length > 0 && (local ? <p className="text-xs text-muted-foreground">Built-in AI uses selected references locally.</p> : <div className="space-y-2">
        <label htmlFor={permissionId} className="flex items-center gap-2 text-sm"><Switch id={permissionId} checked={state.sharingEnabled} disabled={!provider||!ready||state.sharingLoading||state.sending||state.creating} onCheckedChange={enabled => void state.setDocumentSharing(enabled)} />Use selected references for answers</label>
        <p className="text-xs text-muted-foreground">Off by default for each conversation. Enabling this sends selected excerpts to {knowledgeProviderLabel(provider)}, your selected answer provider. Change the provider in Summary settings. Local search and preview remain available.</p>
        {state.sharingLoading && <p role="status" className="text-xs text-muted-foreground">Checking reference sharing…</p>}
        {state.sharingError && <p role="alert" className="text-sm text-destructive">{state.sharingError}</p>}
      </div>)}
      <Button variant="ghost" size="sm" disabled={panelBusy||state.documentsLoading} onClick={state.reloadDocuments}>Reload references</Button>
    </div>}
    <Dialog open={!!preview} onOpenChange={value => {if(!value){previewGeneration.current++;setPreview(null);setPreviewBusy(false);}}}><DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-2xl"><DialogHeader><DialogTitle>{preview?.row.attachment.display_name ?? 'Reference preview'}</DialogTitle><DialogDescription>Locally extracted text with actual page and paragraph anchors.</DialogDescription></DialogHeader>
      {previewBusy && <p role="status">Loading extracted text…</p>}{previewError && <p role="alert" className="text-sm text-destructive">{previewError}</p>}
      {preview?.blocks.map((block,index) => <section key={index} className="space-y-1"><h3 className="text-xs font-medium text-muted-foreground">{documentAnchor({kind:'document',document_id:preview.row.attachment.id,page:block.page,paragraph:block.paragraph})}</h3><p className="whitespace-pre-wrap break-words text-sm">{block.text}</p></section>)}
      {previewError && preview && <Button variant="outline" disabled={previewBusy} onClick={() => void inspect(preview.row)}>Retry preview</Button>}
    </DialogContent></Dialog>
    <Dialog open={!!deleteTarget} onOpenChange={value => {if(!value&&!busy)setDeleteTarget(null);}}><DialogContent><DialogHeader><DialogTitle>Delete reference permanently?</DialogTitle><DialogDescription>Delete “{deleteTarget?.attachment.display_name}”, its original file and extracted text from every attached meeting. This document may be shared with other meetings. Saved citations will become unavailable.</DialogDescription></DialogHeader>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}<div className="flex flex-wrap justify-end gap-2"><Button variant="outline" disabled={busy} onClick={() => setDeleteTarget(null)}>Cancel</Button><Button variant="destructive" disabled={busy||!deleteTarget} onClick={() => {if(deleteTarget)void change(deleteTarget,'delete');}}>{busy?'Deleting…':'Delete permanently'}</Button></div>
    </DialogContent></Dialog>
  </section>;
}
