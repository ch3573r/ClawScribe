'use client';
import { useState } from 'react';
import { Button } from '@/components/ui/button';
import { citationParts,contextLinks } from '@/lib/knowledge-state';
import type { KnowledgeSearchState } from '@/hooks/useKnowledgeSearch';
import type { AssistantReply, SearchMode } from '@/types/knowledge';
function CitedAnswer({reply,state}:{reply:AssistantReply;state:KnowledgeSearchState}) {
  return <><div className="whitespace-pre-wrap break-words text-sm leading-6">{citationParts(reply.content,reply.evidence.length,reply.cited_tags).map((part,index)=>part.tags?
    <span key={index} aria-label={part.text}>{part.tags.map((tag,n)=><button key={`${tag}-${n}`} className="mx-0.5 rounded text-primary underline decoration-primary/40 underline-offset-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" onClick={()=>void state.inspect(reply.evidence[tag-1],reply.evidence_metadata[tag-1])}>[K{tag}]</button>)}</span>:<span key={index}>{part.text}</span>)}</div>
    {contextLinks(reply).map((link,index)=><p key={index} className="mt-2 text-xs text-muted-foreground">[K{link.cited_tag}] · <button className="rounded text-primary underline focus-visible:ring-2 focus-visible:ring-ring" onClick={()=>void state.inspect(reply.evidence[link.context_tag-1],reply.evidence_metadata[link.context_tag-1],reply.evidence[link.cited_tag-1])}>Preceding question context [K{link.context_tag}]</button></p>)}
    <p className="mt-2 text-xs text-muted-foreground">Answered by {reply.provider} · {reply.model} · {reply.retrieval_mode==='hybrid'?'Semantic + keyword':'Keyword'} retrieval</p></>;
}
export function KnowledgeChat({state,mode,provider,model,title='Ask about these meetings'}:{state:KnowledgeSearchState;mode:SearchMode;provider?:string;model?:string;title?:string}) {
  const [question,setQuestion]=useState('');
  const ready=!!provider&&!!model&&!!state.owner&&!state.historyLoading;
  const submit=()=>{if(question.trim()&&ready&&!state.sending)void state.ask(question,mode);};
  return <section className="flex min-h-0 flex-col rounded-lg border border-border bg-card" aria-label={title}>
    <header className="space-y-2 border-b border-border p-4"><h2 className="font-semibold">{title}</h2><p className="text-xs text-muted-foreground">Configured answer provider: {provider||'Not configured'}{model?` · ${model}`:''}</p>
      <p className="text-xs text-muted-foreground">{state.scope.kind==='meeting'?'Only this meeting’s transcript.':'Answers use the explicit meeting and project selection above.'} Check cited sources before relying on an answer.</p>
      {state.scope.kind==='library'&&<div className="flex flex-wrap gap-2"><Button variant="outline" size="sm" disabled={state.sending} onClick={()=>void state.newConversation()}>New conversation</Button>
        <label className="flex items-center gap-2 text-xs">Saved conversation <select aria-label="Saved library conversation" className="max-w-40 rounded border border-input bg-background p-2" value={state.owner?.id??''} onChange={e=>{const owner=state.threads.find(row=>row.id===e.target.value);if(owner)state.selectConversation(owner);}}><option value="">Choose a conversation</option>{state.threads.map((owner,index)=><option key={owner.id} value={owner.id}>Conversation {state.threads.length-index}</option>)}</select></label></div>}
      {!!state.messages.length&&<Button variant="ghost" size="sm" disabled={state.historyLoading} onClick={()=>void state.clear()}>Clear conversation</Button>}
    </header>
    <div className="max-h-[30rem] flex-1 space-y-4 overflow-y-auto p-4" aria-live="polite">
      {state.historyLoading&&<p role="status" className="text-sm text-muted-foreground">Loading saved conversation…</p>}
      {!state.messages.length&&!state.historyLoading&&<p className="text-sm text-muted-foreground">Ask about decisions, action items, or what was said. {state.scope.kind==='library'&&!state.owner?'Create or choose a conversation to begin.':''}</p>}
      {state.messages.map(message=><article key={message.id} className={`rounded-lg p-3 ${message.role==='user'?'ml-6 bg-primary/10':'mr-2 bg-muted'}`}>
        <p className="mb-1 text-xs font-medium text-muted-foreground">{message.role==='user'?'You':'Assistant'}{message.legacy?' · Earlier meeting chat':''}{message.status!=='completed'?` · ${message.status}`:''}</p>
        {message.reply?<CitedAnswer reply={message.reply} state={state}/>:<p className="whitespace-pre-wrap break-words text-sm">{message.content}</p>}
      </article>)}
      {state.sending&&<p role="status" className="text-sm text-muted-foreground">Retrieving evidence and answering…</p>}
    </div>
    {state.error&&<p role="alert" className="px-4 py-2 text-sm text-destructive">{state.error}</p>}
    <form className="space-y-3 border-t border-border p-4" onSubmit={event=>{event.preventDefault();submit();}}><label className="block text-xs font-medium" htmlFor={`knowledge-question-${state.owner?.id??'library'}`}>Question</label>
      <textarea id={`knowledge-question-${state.owner?.id??'library'}`} value={question} onChange={e=>setQuestion(e.target.value)} rows={2} placeholder="What did we decide?" className="w-full resize-y rounded-md border border-input bg-background p-3 text-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" onKeyDown={e=>{if(e.key==='Enter'&&!e.shiftKey){e.preventDefault();submit();}}}/>
      <div className="flex flex-wrap items-center gap-2"><Button type="submit" disabled={!ready||state.sending||!question.trim()}>Ask</Button>{state.sending&&<Button type="button" variant="outline" onClick={state.cancel}>Cancel answer</Button>}<span className="text-xs text-muted-foreground">Shift+Enter for a new line</span></div>
      {!provider||!model?<p className="text-xs text-muted-foreground">Choose a summary provider and model in Settings to answer questions.</p>:null}
    </form>
  </section>;
}
