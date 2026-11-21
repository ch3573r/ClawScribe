'use client';
import { useEffect,useRef,useState } from 'react';
import { Button } from '@/components/ui/button';
import { evidenceIntent,finishEvidenceIntent,navigateEvidence } from '@/lib/knowledge-navigation';
import { knowledgeService } from '@/services/knowledgeService';
export function EvidenceRoute({token,meetingId,audioReady,audioStatus,audioError,onReveal,onPlay}:{
  token:string|null|undefined;meetingId:string;audioReady:boolean;audioStatus:string;audioError:string|null;
  onReveal:(id:string,index:number,current:()=>boolean)=>Promise<void>;onPlay:(seconds:number,current:()=>boolean)=>Promise<void>;
}) {
  const completed=useRef('');const [error,setError]=useState('');const [busy,setBusy]=useState(false);const [retry,setRetry]=useState(0);
  const [waitExpired,setWaitExpired]=useState(false);
  useEffect(()=>{setWaitExpired(false);if(!token)return;const timer=setTimeout(()=>setWaitExpired(true),10000);return()=>clearTimeout(timer);},[token]);
  useEffect(()=>{
    let active = true;
    const current = () => active;
    const cancel = () => { active = false; };
    if(!token||completed.current===token)return;
    const intent=evidenceIntent(token,meetingId);
    if(!intent){setBusy(false);setError('This source link is no longer available. Open the source again.');return;}
    if(intent.play&&!audioReady&&!audioError&&audioStatus!=='missing'&&!waitExpired){setBusy(true);return cancel;}
    setError('');setBusy(true);
    void navigateEvidence(intent.reference,intent.play,{current,resolve:knowledgeService.resolve,reveal:onReveal,play:onPlay},intent.contextReference)
      .then(value=>{if(!current()||!value)return;completed.current=token;finishEvidenceIntent(token);setBusy(false);})
      .catch(e=>{if(current()){setBusy(false);setError(e instanceof Error?e.message:'Could not open the source. Retry.');}});
    return cancel;
  },[token,meetingId,audioReady,audioStatus,audioError,onReveal,onPlay,retry,waitExpired]);
  if(!token||(!busy&&!error))return null;
  const intent = evidenceIntent(token, meetingId);
  return <div className="flex flex-wrap items-center gap-3 border-b border-border bg-card px-4 py-3 text-sm">{busy&&<p role="status">Opening transcript passage{intent?.play&&!audioReady?' · waiting for saved audio':''}…</p>}{error&&<><p role="alert" className="text-destructive">{error}</p>{intent && <Button variant="outline" size="sm" onClick={()=>setRetry(value=>value+1)}>Retry opening passage</Button>}</>}</div>;
}
