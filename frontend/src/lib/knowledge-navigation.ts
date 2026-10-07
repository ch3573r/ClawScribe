import type { EvidenceRef, ResolvedEvidence } from '@/types/knowledge';
// One bounded, ephemeral handoff. No transcript text or private paths in the URL.
let intent:{token:string;reference:EvidenceRef;play:boolean;contextReference?:EvidenceRef}|null=null;
export function createEvidenceIntent(reference:EvidenceRef,play:boolean,contextReference?:EvidenceRef):string {
  const token=globalThis.crypto.randomUUID();intent={token,reference,play,contextReference};return token;
}
export function evidenceIntent(token:string,meetingId:string) {
  if(intent?.token!==token||intent.reference.locator.kind!=='transcript'||intent.reference.locator.meeting_id!==meetingId)return null;
  return intent;
}
export function finishEvidenceIntent(token:string) {if(intent?.token===token)intent=null;}

/** Every awaited stage carries the route/action generation to the existing paginator and player. */
export async function navigateEvidence(reference:EvidenceRef,play:boolean,operations:{
  current:()=>boolean;resolve:(reference:EvidenceRef)=>Promise<ResolvedEvidence>;
  reveal:(id:string,index:number,current:()=>boolean)=>Promise<void>;
  play:(seconds:number,current:()=>boolean)=>Promise<void>;
},contextReference?:EvidenceRef) {
  const {current}=operations;
  if(!current())return;
  if(contextReference){const context=await operations.resolve(contextReference);if(!current())return;if(context.status!=='current'||!context.navigation)throw Error('The originating citation changed. Ask again for fresh question context.');}
  const resolved=await operations.resolve(reference);if(!current())return;
  if(resolved.status!=='current'||!resolved.navigation||!resolved.passage)throw Error(resolved.status==='stale'?'This reference is historical or changed. Ask again for fresh evidence.':'This source is missing or could not be verified.');
  const target=resolved.navigation;
  if(reference.locator.kind!=='transcript'||target.meeting_id!==reference.locator.meeting_id||!Number.isInteger(target.transcript_index)||target.transcript_index<0)throw Error('The current source position could not be verified.');
  await operations.reveal(target.transcript_id,target.transcript_index,current);if(!current())return;
  if(play){if(target.start_seconds===null||!Number.isFinite(target.start_seconds)||target.start_seconds<0)throw Error('This citation has no known recording offset.');await operations.play(target.start_seconds,current);if(!current())return;}
  return resolved;
}
