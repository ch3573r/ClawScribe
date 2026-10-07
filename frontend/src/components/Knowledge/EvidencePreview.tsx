'use client';
import { useRouter } from 'next/navigation';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { Button } from '@/components/ui/button';
import { Dialog,DialogContent,DialogDescription,DialogHeader,DialogTitle } from '@/components/ui/dialog';
import { createEvidenceIntent } from '@/lib/knowledge-navigation';
import { knowledgeService } from '@/services/knowledgeService';
import type { KnowledgeSearchState } from '@/hooks/useKnowledgeSearch';
export function EvidencePreview({state,onNavigate}:{state:KnowledgeSearchState;onNavigate?:()=>void}) {
  const router=useRouter();const {setCurrentMeeting}=useSidebar();
  async function navigate(play:boolean) {
    const preview=state.preview;if(!preview||state.previewBusy)return;
    const current=state.controller.ticket('preview-operation');
    try {
      if(preview.contextReference){const origin=await knowledgeService.resolve(preview.contextReference);if(!current())return;if(origin.status!=='current'){await state.inspect(preview.reference,preview.metadata,preview.contextReference);return;}}
      // Re-resolve at the action boundary; the preview can have been open during an edit.
      const resolved=await knowledgeService.resolve(preview.reference);if(!current())return;
      if(resolved.status!=='current'||!resolved.navigation||!resolved.passage){await state.inspect(preview.reference,preview.metadata,preview.contextReference);return;}
      if(play&&resolved.navigation.start_seconds===null)return;
      const token=createEvidenceIntent(preview.reference,play,preview.contextReference);
      setCurrentMeeting({id:resolved.navigation.meeting_id,title:resolved.passage.title});
      router.push(`/meeting-details?id=${encodeURIComponent(resolved.navigation.meeting_id)}&evidence=${encodeURIComponent(token)}`);
      onNavigate?.();
      state.closePreview();
    }catch{if(current())void state.inspect(preview.reference,preview.metadata,preview.contextReference);}
  }
  const preview=state.preview;const resolved=preview?.resolved;const valid=resolved?.status==='current'&&!!resolved.navigation;
  return <Dialog open={!!preview} onOpenChange={open=>{if(!open)state.closePreview();}}>
    <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-xl"><DialogHeader>
      <DialogTitle>Source passage</DialogTitle><DialogDescription>Review the original evidence, then reveal the current transcript or play its saved recording.</DialogDescription>
    </DialogHeader>
    {preview&&<><h3 className="break-words font-semibold">{preview.metadata.title}</h3><p className="text-xs text-muted-foreground">Original source · {preview.metadata.date}{preview.metadata.speaker?` · ${preview.metadata.speaker}`:''}</p></>}
    {(preview?.metadata.metadata_truncated||resolved?.passage?.metadata_truncated)&&<p className="text-xs text-muted-foreground">Source labels were shortened. Inspect the transcript for the complete meeting context.</p>}
    {state.previewBusy&&<p role="status">Resolving canonical source…</p>}
    {state.error&&<p role="alert" className="text-sm text-destructive">{state.error}</p>}
    {resolved&&<p role="status" className="text-sm text-muted-foreground">{resolved.status==='current'?'Current verified source.':resolved.status==='stale'?'This reference is historical or the source changed. Ask again to retrieve fresh evidence.':resolved.status==='missing'?'The source was deleted or is unavailable. Ask again using the remaining meetings.':'This source reference could not be verified. It cannot navigate.'}</p>}
    {resolved?.passage&&<><p className="text-xs text-muted-foreground">Current source · {resolved.passage.title} · {resolved.passage.date}</p><blockquote className="whitespace-pre-wrap break-words border-l-2 border-primary pl-3 text-sm">{resolved.passage.text}</blockquote></>}
    <div className="flex flex-wrap gap-2"><Button disabled={!valid||state.previewBusy} onClick={()=>void navigate(false)}>Show in transcript</Button>
      <Button variant="outline" disabled={!valid||state.previewBusy||resolved?.navigation?.start_seconds==null} onClick={()=>void navigate(true)}>Play from here</Button>
      {preview&&<Button variant="ghost" disabled={state.previewBusy} onClick={()=>void state.inspect(preview.reference,preview.metadata,preview.contextReference)}>Retry</Button>}</div>
    <p className="text-xs text-muted-foreground">Playback requires this meeting’s saved audio. Unknown audio offsets cannot play from a citation.</p>
    </DialogContent>
  </Dialog>;
}
