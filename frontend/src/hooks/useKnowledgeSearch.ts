'use client';
import { useEffect, useRef, useState } from 'react';
import { createKnowledgeController } from '@/lib/knowledge-state';
import { knowledgeService } from '@/services/knowledgeService';
import type { ConversationOwner, EvidenceDisplay, EvidenceRef, HistoryMessage, KnowledgeScope, ResolvedEvidence, SearchMode, SearchResponse } from '@/types/knowledge';

const errorText=(error:unknown)=> typeof error==='string'?error:error instanceof Error?error.message:'Meeting memory is unavailable. Retry the operation.';
export function useKnowledgeSearch(scope:KnowledgeScope, initialOwner:ConversationOwner|null=null) {
  const [controller] = useState(()=>createKnowledgeController(id=>{void knowledgeService.cancel(id).catch(()=>{});}));
  const [owner,setOwner]=useState<ConversationOwner|null>(initialOwner);
  const [threads,setThreads]=useState<ConversationOwner[]>([]);
  const [response,setResponse]=useState<SearchResponse|null>(null);
  const [messages,setMessages]=useState<HistoryMessage[]>([]);
  const [loading,setLoading]=useState(false);const [sending,setSending]=useState(false);const [historyLoading,setHistoryLoading]=useState(false);
  const [error,setError]=useState('');
  const [preview,setPreview]=useState<{reference:EvidenceRef;metadata:EvidenceDisplay;resolved:ResolvedEvidence|null}|null>(null);
  const [previewBusy,setPreviewBusy]=useState(false);
  const identity=JSON.stringify({scope,owner}); controller.configure({scope,owner});
  const initialKey=JSON.stringify(initialOwner);const initialRef=useRef(initialKey);
  useEffect(()=>{if(initialRef.current!==initialKey){initialRef.current=initialKey;setOwner(initialOwner);}},[initialKey,initialOwner]);
  const reset=()=>{controller.invalidate();setLoading(false);setSending(false);setPreview(null);setPreviewBusy(false);setError('');};
  useEffect(()=>{
    setResponse(null);setPreview(null);setPreviewBusy(false);setMessages([]);setLoading(false);setSending(false);setError('');
    if(!owner){setHistoryLoading(false);return;}
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
    return ()=>controller.invalidate();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  },[scope.kind,controller]);
  async function search(query:string,mode:SearchMode) {
    reset();setResponse(null);if(!query.trim())return;
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
    setSending(true);setError('');const current=controller.ticket('ask-operation');
    try {
      await controller.submit(question.trim(),request_id=>knowledgeService.ask({request_id,owner,search:{scope,query:question.trim(),mode,document_ids:[]}}),reply=>{
        // History is the durable authority for the user turn and exactly one assistant.
        setMessages(rows=>[...rows.filter(row=>row.id!==reply.message_id),{id:reply.message_id,request_id:reply.request_id,role:'assistant',content:reply.content,created_at:'',status:'completed',legacy:false,reply}]);
      });
      if(!current())return;
      const history=await knowledgeService.history(owner);if(!current())return;setMessages(history);
    }catch(e){if(current())setError(errorText(e));}
    finally{if(current())setSending(false);}
  }
  async function clear() {
    if(!owner)return;reset();setHistoryLoading(true);const current=controller.ticket('clear-operation');
    try{await knowledgeService.clear(owner);if(!current())return;setMessages([]);}
    catch(e){if(current())setError(errorText(e));}
    finally{if(current())setHistoryLoading(false);}
  }
  async function inspect(reference:EvidenceRef,metadata:EvidenceDisplay,contextReference?:EvidenceRef) {
    setPreview({reference,metadata,resolved:null});setPreviewBusy(true);setError('');const current=controller.ticket('preview-operation');
    try{
      if(contextReference){const origin=await knowledgeService.resolve(contextReference);if(!current())return;if(origin.status!=='current'){setPreview({reference,metadata,resolved:{status:origin.status,passage:null,navigation:null}});return;}}
      const resolved=await knowledgeService.resolve(reference);if(!current())return;setPreview({reference,metadata,resolved});
    }catch(e){if(current())setError(errorText(e));}
    finally{if(current())setPreviewBusy(false);}
  }
  return {scope,owner,threads,response,messages,loading,sending,historyLoading,error,preview,previewBusy,controller,search,ask,clear,inspect,newConversation,
    cancel:reset,closePreview:()=>{controller.ticket('preview-operation');setPreview(null);setPreviewBusy(false);},
    selectConversation:(value:ConversationOwner)=>{reset();setOwner(value);},
  };
}
export type KnowledgeSearchState = ReturnType<typeof useKnowledgeSearch>;
