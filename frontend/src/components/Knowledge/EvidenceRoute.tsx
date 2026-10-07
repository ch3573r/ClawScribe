'use client';
import { useEffect,useRef,useState } from 'react';
import { Button } from '@/components/ui/button';
import { evidenceIntent,finishEvidenceIntent,navigateEvidence } from '@/lib/knowledge-navigation';
import { knowledgeService } from '@/services/knowledgeService';
export function EvidenceRoute({token,meetingId,audioReady,audioStatus,audioError,onReveal,onPlay}:{
  token:string|null|undefined;meetingId:string;audioReady:boolean;audioStatus:string;audioError:string|null;
  onReveal:(id:string,index:number,current:()=>boolean)=>Promise<void>;onPlay:(seconds:number,current:()=>boolean)=>Promise<void>;
}) {
  const generation=useRef(0);const completed=useRef('');const [error,setError]=useState('');const [busy,setBusy]=useState(false);const [retry,setRetry]=useState(0);
  const [waitExpired,setWaitExpired]=useState(false);
  useEffect(()=>{setWaitExpired(false);if(!token)return;const timer=setTimeout(()=>setWaitExpired(true),10000);return()=>clearTimeout(timer);},[token]);
  useEffect(()=>{
    const request=++generation.current;const current=()=>generation.current===request;
    if(!token||completed.current===token)return;
    const intent=evidenceIntent(token,meetingId);
    if(!intent){setError('This citation handoff is no longer available. Open the source reference again.');return;}
    if(intent.play&&!audioReady&&!audioError&&audioStatus!=='missing'&&!waitExpired){setBusy(true);return()=>{generation.current++;};}
    setError('');setBusy(true);
    void navigateEvidence(intent.reference,intent.play,{current,resolve:knowledgeService.resolve,reveal:onReveal,play:onPlay},intent.contextReference)
      .then(value=>{if(!current()||!value)return;completed.current=token;finishEvidenceIntent(token);setBusy(false);})
      .catch(e=>{if(current()){setBusy(false);setError(e instanceof Error?e.message:'Could not open the source. Retry.');}});
    return()=>{generation.current++;};
  },[token,meetingId,audioReady,audioStatus,audioError,onReveal,onPlay,retry,waitExpired]);
  if(!token||(!busy&&!error))return null;
  return <div className="flex flex-wrap items-center gap-3 border-b border-border bg-card px-4 py-3 text-sm">{busy&&<p role="status">Opening cited transcript{!audioReady?'; waiting for saved audio':''}…</p>}{error&&<><p role="alert" className="text-destructive">{error}</p><Button variant="outline" size="sm" onClick={()=>setRetry(value=>value+1)}>Retry citation</Button></>}</div>;
}
