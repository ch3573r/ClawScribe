'use client';
import { useEffect, useState } from 'react';
import { createKnowledgeController,scopeReady } from '@/lib/knowledge-state';
import { knowledgeService } from '@/services/knowledgeService';
import type { ConversationOwner, EvidenceDisplay, EvidenceRef, HistoryMessage, KnowledgeScope, ResolvedEvidence, SearchMode, SearchResponse } from '@/types/knowledge';

const errorText=(error:unknown)=> typeof error==='string'?error:error instanceof Error?error.message:'Meeting memory is unavailable. Retry the operation.';
export function useKnowledgeSearch(scope:KnowledgeScope, initialOwner:ConversationOwner|null=null) {
  const [controller] = useState(()=>createKnowledgeController(id=>{void knowledgeService.cancel(id).catch(()=>{});}));
  const [savedOwner,setOwner]=useState<ConversationOwner|null>(initialOwner);
  const owner=initialOwner?.kind==='meeting'?initialOwner:savedOwner;
  const [threads,setThreads]=useState<ConversationOwner[]>([]);
  const [response,setResponse]=useState<SearchResponse|null>(null);
  const [messages,setMessages]=useState<HistoryMessage[]>([]);
  const [loading,setLoading]=useState(false);const [sending,setSending]=useState(false);const [historyLoading,setHistoryLoading]=useState(false);
  const [error,setError]=useState('');
  const [preview,setPreview]=useState<{reference:EvidenceRef;metadata:EvidenceDisplay;resolved:ResolvedEvidence|null;contextReference?:EvidenceRef}|null>(null);
  const [previewBusy,setPreviewBusy]=useState(false);
  const identity=JSON.stringify({scope,owner}); controller.configure({scope,owner});
  const [visibleFor,setVisibleFor]=useState(identity);
  const reset=()=>{controller.invalidate();setResponse(null);setLoading(false);setSending(false);setHistoryLoading(false);setPreview(null);setPreviewBusy(false);setError('');};
  useEffect(()=>{
    setVisibleFor(identity);
    setResponse(null);setPreview(null);setPreviewBusy(false);setMessages([]);setLoading(false);setSending(false);setError('');
    if(!owner){setHistoryLoading(false);return ()=>controller.invalidate();}
    setHistoryLoading(true);const current=controller.ticket('history-operation');
    void controller.run('history',()=>knowledgeService.history(owner),value=>{setMessages(value);setHistoryLoading(false);})
      .catch(e=>{if(current()){setError(errorText(e));setHistoryLoading(false);}});
    return ()=>controller.invalidate();
  // JSON identity keeps equal scope objects from restarting history on every render.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  },[identity,controller]);
  useEffect(()=>{
    if(scope.kind!=='library')return;
    const current=controller.ticket('threads-operation');
    void controller.run('threads',knowledgeService.conversations,setThreads).catch(e=>{if(current())setError(errorText(e));});
  // eslint-disable-next-line react-hooks/exhaustive-deps
  },[identity,controller]);
  async function search(query:string,mode:SearchMode) {
    reset();setResponse(null);if(!query.trim())return;
    if(!scopeReady(scope)){setError('Select one or more meetings, or explicitly choose all saved meetings. Check the date range.');return;}
    setLoading(true);const current=controller.ticket('search-operation');
    try {await controller.run('search',()=>knowledgeService.search({scope,query:query.trim(),mode,document_ids:[]}),value=>{setResponse(value);setLoading(false);});}
    catch(e){if(current()){setError(errorText(e));setLoading(false);}}
  }
  async function newConversation() {
    reset();const current=controller.ticket('new-conversation');
    try {const value=await knowledgeService.createConversation();if(!current())return;setThreads(rows=>[value,...rows]);setOwner(value);}
    catch(e){if(current())setError(errorText(e));}
  }
  async function ask(question:string,mode:SearchMode) {
    if(!question.trim()||!owner||historyLoading)return;
    if(controller.pendingRequest())return;
    if(!scopeReady(scope)){setError('Select one or more meetings, or explicitly choose all saved meetings. Check the date range.');return;}
    setSending(true);setError('');const current=controller.ticket('ask-operation');
    try {
      await controller.submit(question.trim(),async request_id=>{
        await knowledgeService.ask({request_id,owner,search:{scope,query:question.trim(),mode,document_ids:[]}});
        if(!current())return null;
        const history=await knowledgeService.history(owner);
        if(!current())return null;
        return history;
      },history=>{
        // Hold the synchronous pending guard through durable history reconciliation.
        if(history)setMessages(history);
      });
    }catch(e){if(current())setError(errorText(e));}
    finally{if(current())setSending(false);}
  }
  async function clear() {
    if(!owner)return;reset();setHistoryLoading(true);const current=controller.ticket('clear-operation');
    try{await knowledgeService.clear(owner);if(!current())return;setMessages([]);}
    catch(e){if(current())setError(errorText(e));}
    finally{if(current())setHistoryLoading(false);}
  }
  async function reloadHistory() {
    if(!owner)return;reset();setHistoryLoading(true);const current=controller.ticket('history-operation');
    try{await controller.run('history',()=>knowledgeService.history(owner),value=>{setMessages(value);setHistoryLoading(false);});}
    catch(e){if(current()){setError(errorText(e));setHistoryLoading(false);}}
  }
  async function inspect(reference:EvidenceRef,metadata:EvidenceDisplay,contextReference?:EvidenceRef) {
    setPreview({reference,metadata,resolved:null,contextReference});setPreviewBusy(true);setError('');const current=controller.ticket('preview-operation');
    try{
      if(contextReference){const origin=await knowledgeService.resolve(contextReference);if(!current())return;if(origin.status!=='current'){setPreview({reference,metadata,contextReference,resolved:{status:origin.status,passage:null,navigation:null}});return;}}
      const resolved=await knowledgeService.resolve(reference);if(!current())return;setPreview({reference,metadata,resolved,contextReference});
    }catch(e){if(current())setError(errorText(e));}
    finally{if(current())setPreviewBusy(false);}
  }
  const visible=visibleFor===identity;
  return {scope,owner,threads,response:visible?response:null,messages:visible?messages:[],loading:visible&&loading,sending:visible&&sending,historyLoading:!visible||historyLoading,error:visible?error:'',preview:visible?preview:null,previewBusy:visible&&previewBusy,controller,search,ask,clear,inspect,newConversation,reloadHistory,
    cancel:reset,closePreview:()=>{controller.ticket('preview-operation');setPreview(null);setPreviewBusy(false);},
    selectConversation:(value:ConversationOwner)=>{reset();setOwner(value);},
  };
}
export type KnowledgeSearchState = ReturnType<typeof useKnowledgeSearch>;
