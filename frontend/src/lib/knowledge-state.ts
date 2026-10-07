import type { AssistantReply, CitationContextLink, KnowledgeScope } from '@/types/knowledge';
import type { ProjectFilter } from '@/lib/library';

/** A single generation owns all awaited work for one search/conversation surface. */
export function createKnowledgeController(onCancel?: (id:string,current:()=>boolean) => void) {
  let generation = 0; let configuration = ''; const lanes = new Map<string,number>();
  let pending: {id:string;promise:Promise<unknown>} | null = null;
  let retry: {question:string;id:string} | null = null;
  const invalidate = () => { generation++; lanes.clear(); const current=generation;if (pending) onCancel?.(pending.id,()=>generation===current); pending = null; retry = null; };
  const ticket = (lane:string) => {
    const current = generation; const sequence = (lanes.get(lane) ?? 0) + 1; lanes.set(lane,sequence);
    return () => generation === current && lanes.get(lane) === sequence;
  };
  return {
    invalidate,
    configure(value:unknown) { const next=JSON.stringify(value); if(next!==configuration){ invalidate(); configuration=next; } },
    ticket,
    pendingRequest: () => pending?.id ?? null,
    async run<T>(lane:string, operation:()=>Promise<T>, commit:(value:T)=>void) {
      const current = ticket(lane);
      try { const value = await operation(); if(current()) { commit(value); return value; } }
      catch(error) { if(current()) throw error; }
    },
    submit<T>(question:string, operation:(id:string)=>Promise<T>, commit:(value:T)=>void, uuid:()=>string=()=>globalThis.crypto.randomUUID()):Promise<unknown> {
      if(pending) return pending.promise;
      const id=retry?.question===question ? retry.id : uuid(); retry={question,id};
      const current=ticket('ask');
      // Install the guard synchronously, before invoking a provider or yielding to React.
      const entry={id,promise:Promise.resolve<unknown>(undefined)}; pending=entry;
      let work:Promise<T>;
      try { work=Promise.resolve(operation(id)); } catch(error) { work=Promise.reject(error); }
      entry.promise=work.then(value=>{ if(current()){commit(value);retry=null;} })
        .catch(error=>{if(current()) throw error;})
        .finally(()=>{if(pending===entry) pending=null;});
      return entry.promise;
    },
  };
}

export function libraryScope(ids:string[], all:boolean, project:ProjectFilter, from='', to=''):KnowledgeScope {
  return {kind:'library',filter:{all_meetings:all,meeting_ids:[...ids],tags:[...project.tags],tag_mode:project.mode,untagged:project.untagged,from:from||null,to:to||null}};
}
export function scopeReady(scope:KnowledgeScope):boolean {
  return scope.kind==='meeting'?!!scope.meeting_id.trim():
    (scope.filter.all_meetings||scope.filter.meeting_ids.length>0)&&!(scope.filter.from&&scope.filter.to&&scope.filter.from>scope.filter.to);
}
export interface CitationPart { text:string; tags?:number[] }
/** Preserve text and original map ordinals; reject a whole malformed outer group. */
export function citationParts(content:string,count:number,allow:number[]):CitationPart[] {
  const parts:CitationPart[]=[]; let start=0; let cursor=0;
  const number=(token:string)=>{const match=/^K([1-9]\d{0,2})$/.exec(token.replace(/^\p{White_Space}+|\p{White_Space}+$/gu,'')); const n=match?Number(match[1]):0; return n<=count&&n>0?n:null;};
  while(cursor<content.length) {
    const open=content.indexOf('[',cursor); if(open<0) break;
    let depth=1;let nested=false;let close=open+1;
    for(;close<content.length;close++){if(content[close]==='['){depth++;nested=true;}if(content[close]===']')depth--;if(depth===0)break;}
    if(depth!==0)break;
    const body=content.slice(open+1,close); let tags:number[]|undefined;
    const bytes=Array.from(body).reduce((sum,char)=>sum+(char.codePointAt(0)!<=0x7f?1:char.codePointAt(0)!<=0x7ff?2:char.codePointAt(0)!<=0xffff?3:4),0);
    if(!nested&&bytes<=1024){
      if(body.includes(',')&&!body.includes('-')){const items=body.split(',').map(number);if(items.length<=64&&items.every(n=>n!==null))tags=items as number[];}
      else if(body.includes('-')&&!body.includes(',')){const items=body.split('-').map(number);if(items.length===2&&items[0]!==null&&items[1]!==null&&items[1]>=items[0]&&items[1]-items[0]<64) tags=Array.from({length:items[1]-items[0]+1},(_,i)=>items[0]!+i);}
      else {const n=number(body);if(n!==null) tags=[n];}
    }
    if(tags&&tags.every(n=>allow.includes(n))){if(open>start)parts.push({text:content.slice(start,open)});parts.push({text:content.slice(open,close+1),tags});start=close+1;}
    cursor=close+1;
  }
  if(start<content.length)parts.push({text:content.slice(start)});
  return parts;
}

/** Validate the persisted context map again; this is never a literal model citation. */
export function contextLinks(reply:AssistantReply):CitationContextLink[] {
  const evidence=reply.evidence;const metadata=reply.evidence_metadata;
  if(evidence.length!==metadata.length||evidence.length>64)return [];
  return (reply.context_links??[]).filter(link=>{
    const a=link.cited_tag-1,b=link.context_tag-1;
    if(link.kind!=='preceding_question'||!Number.isInteger(a)||!Number.isInteger(b)||a<0||b<0||a>=evidence.length||b>=evidence.length||a===b||!reply.cited_tags.includes(a+1)||metadata[a].preceding_question_tag!==b+1||metadata[b].preceding_question_tag!=null) return false;
    const x=evidence[a],y=evidence[b]; const xl=x.locator,yl=y.locator;
    if(x.source_id!==y.source_id||x.source_revision!==y.source_revision||!!x.historical!==!!y.historical||xl.kind!=='transcript'||yl.kind!=='transcript'||xl.meeting_id!==yl.meeting_id||x.source_id!==`meeting:${xl.meeting_id}`)return false;
    return xl.transcript_ids.length===1&&yl.transcript_ids.length===1&&xl.transcript_ids[0]!==yl.transcript_ids[0]&&xl.spans.length===1&&yl.spans.length===1&&
      xl.spans[0].transcript_id===xl.transcript_ids[0]&&yl.spans[0].transcript_id===yl.transcript_ids[0]&&xl.spans[0].start_byte===0&&yl.spans[0].start_byte===0&&xl.spans[0].end_byte>0&&xl.spans[0].end_byte<=64&&yl.spans[0].end_byte>0&&yl.spans[0].end_byte<=16384;
  });
}
