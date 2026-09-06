import assert from 'node:assert/strict';
import test from 'node:test';
import { verifyWorkflowPolicy, workflowPolicyErrors } from './verify-workflow-policy.mjs';

test('repository workflows preserve the designated runner and storage policy', () => {
  assert.deepEqual(verifyWorkflowPolicy(), []);
});
test('rejects hosted Windows jobs and remote cache/artifact storage', () => {
  for (const source of ['runs-on: windows-2022', 'uses: actions/upload-artifact@v4',
    'uses: actions/cache@v4', 'cache: pnpm', 'cache: true']) {
    assert.ok(workflowPolicyErrors('fixture.yml', source).length);
  }
});
test('public hosted source validation remains allowed', () => {
  assert.deepEqual(workflowPolicyErrors('fixture.yml', 'runs-on: ubuntu-latest\ncache: false'), []);
});
