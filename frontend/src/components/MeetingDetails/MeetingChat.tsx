'use client';
import { useMemo, useState } from 'react';
import { MessageSquare } from 'lucide-react';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle, DialogTrigger } from '@/components/ui/dialog';
import { KnowledgeChat } from '@/components/Knowledge/KnowledgeChat';
import { EvidencePreview } from '@/components/Knowledge/EvidencePreview';
import { useKnowledgeSearch } from '@/hooks/useKnowledgeSearch';
import { libraryScope } from '@/lib/knowledge-state';
import type { ProjectFilter } from '@/lib/library';
import type { KnowledgeScope, SearchMode } from '@/types/knowledge';

export function MeetingChat({meetingId,provider,model}:{meetingId:string;provider?:string;model?:string}) {
  const [open,setOpen]=useState(false);const [expanded,setExpanded]=useState(false);
  const [selected,setSelected]=useState<string[]>([meetingId]);const [all,setAll]=useState(false);
  const [project,setProject]=useState<ProjectFilter>({tags:[],untagged:false,mode:'any'});
  const [mode,setMode]=useState<SearchMode>('keyword');
  const {meetings,projectTags,projectTagsError,projectTagsLoading}=useSidebar();
  const tags=[...new Set(projectTags.map(row=>row.tag))];
  const scope=useMemo<KnowledgeScope>(()=>expanded?libraryScope(selected,all,project):{kind:'meeting',meeting_id:meetingId},[expanded,selected,all,project,meetingId]);
  const state=useKnowledgeSearch(scope,{kind:'meeting',id:meetingId});
  return <>
    <Dialog open={open} onOpenChange={value=>{setOpen(value);if(!value)state.cancel();}}>
      <DialogTrigger asChild><button aria-label="Chat with this meeting" className="fixed bottom-6 right-6 z-40 flex h-12 w-12 items-center justify-center rounded-full bg-primary text-primary-foreground shadow-lg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"><MessageSquare className="h-5 w-5"/></button></DialogTrigger>
      <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-xl"><DialogHeader><DialogTitle>Chat with this meeting</DialogTitle><DialogDescription>Ask cited questions using your configured summary provider.</DialogDescription></DialogHeader>
        <fieldset className="space-y-3 rounded-lg border border-border p-3"><legend className="px-1 text-sm font-medium">Question scope</legend>
          <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={expanded} onChange={e=>setExpanded(e.target.checked)}/>Include other saved meetings</label>
          {expanded&&<><label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={all} onChange={e=>setAll(e.target.checked)}/>Search all saved meetings</label>
            {!all&&<div className="max-h-32 space-y-2 overflow-y-auto">{meetings.map(meeting=><label key={meeting.id} className="flex items-start gap-2 text-sm"><input type="checkbox" className="mt-1" checked={selected.includes(meeting.id)} onChange={e=>setSelected(ids=>e.target.checked?[...ids,meeting.id]:ids.filter(id=>id!==meeting.id))}/><span className="break-words">{meeting.title}</span></label>)}</div>}
            <label className="block text-xs">Projects (Ctrl or Command selects several)<select multiple aria-label="Question project filters" disabled={projectTagsLoading||!!projectTagsError||project.untagged} value={project.tags} className="mt-1 block w-full rounded border border-input bg-background p-2 text-sm" onChange={e=>setProject(current=>({...current,tags:Array.from(e.target.selectedOptions,option=>option.value)}))}>{tags.map(tag=><option key={tag} value={tag}>{tag}</option>)}</select></label>
            <div className="flex flex-wrap items-center gap-3"><label className="text-xs">Match <select value={project.mode} className="rounded border border-input bg-background p-2" onChange={e=>setProject(current=>({...current,mode:e.target.value as 'any'|'all'}))}><option value="any">Any project</option><option value="all">All projects</option></select></label><label className="flex items-center gap-2 text-xs"><input type="checkbox" checked={project.untagged} onChange={e=>setProject(current=>({...current,tags:[],untagged:e.target.checked}))}/>Untagged only</label></div>
            {projectTagsError&&<p role="alert" className="text-xs text-destructive">Project filters are unavailable. Retry from Meetings.</p>}<p className="text-xs text-muted-foreground">An empty meeting selection searches no meetings.</p>
          </>}
          <label className="flex items-center gap-2 text-xs">Retrieval <select value={mode} className="rounded border border-input bg-background p-2" onChange={e=>{state.cancel();setMode(e.target.value as SearchMode);}}><option value="keyword">Keyword</option><option value="hybrid">Semantic + keyword</option></select></label>
        </fieldset>
        <KnowledgeChat state={state} mode={mode} provider={provider} model={model} title="Ask this meeting"/>
      </DialogContent>
    </Dialog>
    <EvidencePreview state={state}/>
  </>;
}
