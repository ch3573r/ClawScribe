'use client';
import { Button } from '@/components/ui/button';
import type { KnowledgeSearchState } from '@/hooks/useKnowledgeSearch';
export function SearchResults({state}:{state:KnowledgeSearchState}) {
  return <section aria-label="Ranked transcript passages" className="rounded-lg border border-border bg-card">
    <div className="border-b border-border p-4"><h2 className="font-semibold">Transcript passages</h2>
      <p className="mt-1 text-xs text-muted-foreground" role="status">{state.loading?'Searching locally…':state.response?`${state.response.passages.length} passages · ${state.response.mode==='hybrid'?'Semantic + keyword':'Keyword'} retrieval`:'Search your selected meetings to find supporting passages.'}</p>
      {state.response?.index_status.reason&&<p className="mt-2 text-sm text-muted-foreground">{state.response.index_status.reason}</p>}
    </div>
    {state.loading&&<div className="p-4"><Button variant="outline" onClick={state.cancel}>Cancel search</Button></div>}
    {state.response&&!state.loading&&state.response.passages.length===0&&<p className="p-5 text-sm text-muted-foreground">No matching passages in this scope. Try different words or select other meetings.</p>}
    <ol className="divide-y divide-border">{state.response?.passages.map((passage,index)=><li key={`${passage.evidence.chunk_id}-${index}`} className="space-y-2 p-5">
      <div className="flex items-start justify-between gap-3"><div className="min-w-0"><h3 className="break-words text-sm font-semibold">{passage.title}</h3><p className="text-xs text-muted-foreground">{passage.date}{passage.speaker?` · ${passage.speaker}`:''} · Ranked {index+1}</p></div>
        <Button variant="outline" size="sm" onClick={()=>void state.inspect(passage.evidence,passage)}>Inspect passage</Button></div>
      <blockquote className="whitespace-pre-wrap break-words border-l-2 border-primary pl-3 text-sm leading-6">{passage.text}</blockquote>
      {passage.metadata_truncated&&<p className="text-xs text-muted-foreground">Source labels were shortened.</p>}
    </li>)}</ol>
  </section>;
}
