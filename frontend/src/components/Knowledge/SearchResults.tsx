'use client';
import { Button } from '@/components/ui/button';
import { PageSection } from '@/components/ui/page-section';
import { documentAnchor } from '@/lib/knowledge-documents';
import type { KnowledgeSearchState } from '@/hooks/useKnowledgeSearch';
const reasonCopy: Record<string, string> = {
  disabled: 'Semantic search is off. Results use keyword search.',
  model_unavailable: 'The search model is unavailable. Results use keyword search.',
  indexing_failed: 'Some meetings could not be indexed. Retry in Settings.',
  paused: 'Semantic indexing is paused. Results use keyword search.',
  indexing: 'Semantic search is still preparing your meetings.',
  busy: 'Recording or other AI work is using the search model. Results use keyword search.',
  cancelled: 'Semantic search was cancelled. Results use keyword search.',
  query_token_limit: 'This query is too long for semantic search. Results use keyword search.',
};
export function SearchResults({state}:{state:KnowledgeSearchState}) {
  if (!state.loading && !state.response) return null;
  return <PageSection aria-label="Source passages">
    <div className="border-b border-border p-4"><h2 className="font-semibold">Source passages</h2>
      <p className="mt-1 text-xs text-muted-foreground" role="status">{state.loading?'Searching locally…':state.response?`${state.response.passages.length} passages · ${state.response.mode==='hybrid'?'Semantic + keyword':'Keyword'} retrieval`:'Search your selected meetings to find supporting passages.'}</p>
      {state.response?.index_status.reason&&<p className="mt-2 text-sm text-muted-foreground">{reasonCopy[state.response.index_status.reason] ?? 'Semantic search is unavailable. Results use keyword search.'}</p>}
    </div>
    {state.loading&&<div className="p-4"><Button variant="outline" onClick={state.cancel}>Cancel search</Button></div>}
    {state.response&&!state.loading&&state.response.passages.length===0&&<p className="p-5 text-sm text-muted-foreground">No matching passages in this scope. Try different words or select other meetings.</p>}
    <ol className="divide-y divide-border">{state.response?.passages.map((passage,index)=><li key={`${passage.evidence.chunk_id}-${index}`} className="space-y-2 p-5">
      <div className="flex items-start justify-between gap-3"><div className="min-w-0"><h3 className="break-words text-sm font-semibold">{passage.title}</h3><p className="text-xs text-muted-foreground">{passage.evidence.locator.kind==='document'?`Reference · ${documentAnchor(passage.evidence.locator)}`:'Transcript'} · {passage.date}{passage.speaker?` · ${passage.speaker}`:''} · Ranked {index+1}</p></div>
        <Button variant="outline" size="sm" onClick={()=>void state.inspect(passage.evidence,passage)}>Inspect passage</Button></div>
      <blockquote className="whitespace-pre-wrap break-words border-l-2 border-primary pl-3 text-sm leading-6">{passage.text}</blockquote>
      {passage.metadata_truncated&&<p className="text-xs text-muted-foreground">Source labels were shortened.</p>}
    </li>)}</ol>
  </PageSection>;
}
