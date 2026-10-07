import type { EvidenceRef } from '@/types/knowledge';
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
