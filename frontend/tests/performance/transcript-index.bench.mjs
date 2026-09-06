/** Isolated algorithm benchmark, not a Windows/meeting/model performance test.
 * Loads the real production baseline callback from a pinned Git object, or an
 * explicitly supplied baseline source file with the identical Git blob hash.
 */
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import crypto from 'node:crypto';
import os from 'node:os';
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {createRequire} from 'node:module';
import {fileURLToPath} from 'node:url';
import {performance} from 'node:perf_hooks';
import {loadTsModule} from '../lib/load-ts-module.mjs';
const require=createRequire(import.meta.url);
const ts=require('typescript');
const frontend=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..');
const root=path.resolve(frontend,'..');
const baselineCommit='3e8ef924ca702a35366738f57b9312c0371236ed';
const baselineBlob='1245a33542c25f7d1a8683f62dfe68adfcade8bc';
const bytes=process.env.BASELINE_TRANSCRIPT_FILE?fs.readFileSync(process.env.BASELINE_TRANSCRIPT_FILE):execFileSync('git',['show',baselineCommit+':frontend/src/contexts/TranscriptContext.tsx'],{cwd:root});
assert.equal(crypto.createHash('sha1').update(Buffer.concat([Buffer.from(`blob ${bytes.length}\0`),bytes])).digest('hex'),baselineBlob,'Baseline must match the inspected production Git object');
const parsed=ts.createSourceFile('baseline.tsx',bytes.toString(),ts.ScriptTarget.Latest,true,ts.ScriptKind.TSX);
let callback;
function visit(node){
  if(ts.isCallExpression(node)&&node.expression.getText(parsed)==='setTranscripts'&&node.arguments.length&&ts.isArrowFunction(node.arguments[0])){
    const text=node.arguments[0].getText(parsed);
    if(text.includes('const existingSequenceIds = new Set')&&text.includes('allNewTranscripts.filter')){
      assert.equal(callback,undefined,'Ambiguous baseline callback');callback=text;
    }
  }
  ts.forEachChild(node,visit);
}
visit(parsed);assert.ok(callback,'Production baseline updater was not found');
const environment=vm.createContext({console:{log(){}},allNewTranscripts:[]});
const baseline=vm.runInContext(`(${callback})`,environment);
const moduleFile=path.join(frontend,'src/lib/orderedTranscripts.ts');
const {OrderedTranscripts}=loadTsModule(moduleFile);
const item=i=>({id:`transcript-${i}`,sequence_id:i,chunk_start_time:i*3,text:`Er kommt um 10 Uhr. Segment ${i}.`,timestamp:'12:00',is_partial:false});
const median=numbers=>[...numbers].sort((a,b)=>a-b)[Math.floor(numbers.length/2)];
function measure(count,optimized){
  const incoming=Array.from({length:count},(_,i)=>[item(i)]);
  globalThis.gc?.();
  let snapshot=[];const store=new OrderedTranscripts();
  const start=performance.now();
  for(const batch of incoming){
    if(optimized)snapshot=store.merge(batch);
    else{environment.allNewTranscripts=batch;snapshot=baseline(snapshot);}
  }
  const milliseconds=performance.now()-start;
  assert.equal(snapshot.length,count);assert.equal(snapshot[0].sequence_id,0);assert.equal(snapshot[count-1].sequence_id,count-1);
  return milliseconds;
}
measure(500,false);measure(500,true);
const workloads=[1000,3000,6000].map(count=>{
  const baselineRuns=[],optimizedRuns=[];
  for(let i=0;i<3;i++){baselineRuns.push(measure(count,false));optimizedRuns.push(measure(count,true));}
  return {sequential_appends:count,baseline_ms:median(baselineRuns),production_index_ms:median(optimizedRuns),baseline_runs_ms:baselineRuns,production_index_runs_ms:optimizedRuns};
});
const result={scope:'Isolated ordered transcript ingestion; excludes React rendering, native audio, IndexedDB, model inference and the meeting application. Not an i5-1235U benchmark.',baseline_commit:baselineCommit,baseline_blob:baselineBlob,index_source_sha256:crypto.createHash('sha256').update(fs.readFileSync(moduleFile)).digest('hex'),node:process.version,platform:os.platform(),architecture:os.arch(),workloads};
console.log(JSON.stringify(result,null,2));
