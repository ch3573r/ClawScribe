import assert from 'node:assert/strict';
import test from 'node:test';
import React from 'react';
import {renderToStaticMarkup} from 'react-dom/server';
import {loadTsModule} from './load-ts-module.mjs';
import {createHookHarness} from './hook-harness.mjs';

const scope={kind:'library',filter:{all_meetings:false,meeting_ids:['meeting'],tags:[],tag_mode:'any',untagged:false,from:null,to:null}};
const reference=n=>({source_id:'meeting:meeting',source_revision:1,chunk_id:`row-${n}`,fingerprint:'fixture',locator:{kind:'transcript',meeting_id:'meeting',transcript_ids:[`row-${n}`],spans:[{transcript_id:`row-${n}`,start_byte:0,end_byte:20}],start_seconds:n}});
const reply=(content,count=2)=>({content,provider:'codex',model:'gpt-6.1-sol',retrieval_mode:'keyword',evidence:Array.from({length:count},(_,i)=>reference(i+1)),evidence_metadata:Array.from({length:count},()=>({title:'Launch review',date:'2026-09-01',speaker:null,preceding_question_tag:null})),cited_tags:Array.from({length:count},(_,i)=>i+1),context_links:[]});
const stateFor=answer=>({scope,owner:{kind:'library',id:'saved'},messages:answer?[{id:1,role:'assistant',content:answer.content,status:'completed',reply:answer,created_at:'2026-09-01'}]:[],threads:[],historyLoading:false,sending:false,creating:false,inspect(){},ask:async()=>{}});
const element=tag=>({children,asChild,node,...props})=>React.createElement(tag,props,children);
const mocks={
  'lucide-react':{Info:element('span'),MoreHorizontal:element('span')},
  '@/services/knowledgeService':{knowledgeService:{history:async()=>[]}},
  '@/components/ui/button':{Button:element('button')},
  '@/components/ui/textarea':{Textarea:element('textarea')},
  '@/components/ui/tooltip':Object.fromEntries(['Tooltip','TooltipTrigger','TooltipContent','TooltipProvider'].map(name=>[name,element('span')])),
  '@/components/ui/dropdown-menu':Object.fromEntries(['DropdownMenu','DropdownMenuContent','DropdownMenuItem','DropdownMenuTrigger','DropdownMenuSeparator','DropdownMenuLabel'].map(name=>[name,element('div')])),
  '@/components/ui/scroll-area':{ScrollArea:element('div')},
};
function rendered(answer) {
  const {KnowledgeChat}=loadTsModule('src/components/Knowledge/KnowledgeChat.tsx',mocks);
  return renderToStaticMarkup(React.createElement(KnowledgeChat,{state:stateFor(answer),mode:'keyword',provider:'codex',model:'gpt-6.1-sol'}));
}
test('meeting answers render bold, lists and independently clickable citations in a list item',()=>{
  const html=rendered(reply('**Decision**\n\n- Ship the plan [K1][K2].'));
  assert.match(html,/<strong>Decision<\/strong>/);
  assert.match(html,/<ul\b/);
  assert.match(html,/<li\b[^>]*>[\s\S]*aria-label="Open source 1"[\s\S]*aria-label="Open source 2"[\s\S]*<\/li>/);
  assert.doesNotMatch(html,/\[K[12]\]/);
  assert.doesNotMatch(html,/codex · gpt-6.1-sol/);
});
test('raw answer HTML remains visible text, never executable markup',()=>{
  const html=rendered(reply('<script>alert("unsafe")</script>\n\n<img src="x" onerror="unsafe">'));
  assert.doesNotMatch(html,/<script|<img/);
  assert.match(html,/&lt;script&gt;/);
  assert.match(html,/&lt;img/);
});
test('budget-sized evidence maps retain citations after the former 64-row limit',()=>{
  const html=rendered(reply('Both meetings are covered [K1][K700].',700));
  assert.match(html,/aria-label="Open source 700"/);
  assert.doesNotMatch(html,/\[K700\]/);
});

test('live short-reply citation renders the verified preceding question without accepting a foreign session', () => {
  const answer = reply('Ja. [K2]');
  answer.evidence = [1, 2].map(sequence => ({ source_id: 'live:public-session', source_revision: 1,
    chunk_id: `live:public-session:${sequence}`, fingerprint: `public-live-${sequence}`,
    locator: { kind: 'live', session_id: 'public-session', sequence_ids: [sequence] } }));
  answer.evidence_metadata[1].preceding_question_tag = 1;
  answer.cited_tags = [2]; answer.context_links = [{ kind: 'preceding_question', cited_tag: 2, context_tag: 1 }];
  answer.live_context = { session_id: 'public-session', finalized_through_seconds: 3540, transcription_incomplete: true };
  assert.match(rendered(answer), /aria-label="Open question for source 2"/);
  const foreign = { ...answer, evidence: [{ ...answer.evidence[0], locator: { ...answer.evidence[0].locator, session_id: 'foreign-session' } }, answer.evidence[1]] };
  assert.doesNotMatch(rendered(foreign), /aria-label="Open question for source 2"/);
});
const jsx=(type,props)=>({type,props});
const nodes=node=>Array.isArray(node)?node.flatMap(nodes):node&&typeof node==='object'?[node,...nodes(node.props?.children)]:[];
test('accepted questions clear immediately and failed requests restore the question',async()=>{
  const hooks=createHookHarness();
  const state=stateFor();
  state.owner=null;
  let submitted='';state.ask=async question=>{submitted=question;state.sending=true;};
  const {KnowledgeChat}=loadTsModule('src/components/Knowledge/KnowledgeChat.tsx',{
    ...mocks,react:hooks.react,'react/jsx-runtime':{jsx,jsxs:jsx},
    '@/components/ui/textarea':{Textarea:'textarea'},
  });
  const render=()=>hooks.render(()=>KnowledgeChat({state,mode:'keyword',provider:'codex',model:'gpt-6.1-sol'}));
  const input=tree=>nodes(tree).find(node=>node.type==='textarea');
  input(render()).props.onChange({target:{value:'What did we decide?'}});
  nodes(render()).find(node=>node.type==='form').props.onSubmit({preventDefault(){}});
  assert.equal(submitted,'What did we decide?');
  assert.equal(input(render()).props.value,'');
  state.sending=false;state.error='Provider unavailable';render();
  assert.equal(input(render()).props.value,'What did we decide?');
  hooks.unmount();
});
