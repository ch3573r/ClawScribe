import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { workflowPolicyErrors } from './verify-workflow-policy.mjs';
const manual = "${{ github.event_name == 'workflow_dispatch' && inputs.knowledge && inputs['answer-acceptance'] }}";
const workflow = readFileSync('.github/workflows/summary-chunking-tests.yml','utf8');
test('exact manual QA condition excludes all pull requests before scheduling',()=>assert.deepEqual(workflowPolicyErrors('manual-qa',workflow),[]));
test('near-match manual QA conditions cannot weaken existing PR guard',()=>{
 for(const unsafe of [manual.replace("== 'workflow_dispatch'","!= 'push'"),manual.replace(' && inputs.knowledge',' || inputs.knowledge')]) {
  assert.ok(workflowPolicyErrors('manual-qa',workflow.replace(manual,unsafe)).some(e=>e.includes('exclude forks')));
 }
});
