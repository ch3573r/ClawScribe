import assert from 'node:assert/strict';
import test from 'node:test';
import { trustedPullRequestCondition, verifyWorkflowPolicy, workflowPolicyErrors } from './verify-workflow-policy.mjs';

const localJob = `    runs-on: [self-hosted, Windows, X64, clawscribe]
    steps:
      - name: Require the designated local build machine
        env:
          EXPECTED_BUILD_RUNNER: \${{ vars.CLAWSCRIBE_BUILD_RUNNER }}
          RUNNER_ENVIRONMENT: \${{ runner.environment }}
        run: |
          if ($env:RUNNER_ENVIRONMENT -ne 'self-hosted' -or
              [string]::IsNullOrWhiteSpace($env:EXPECTED_BUILD_RUNNER) -or
              $env:RUNNER_NAME -ine $env:EXPECTED_BUILD_RUNNER -or
              $env:COMPUTERNAME -ine $env:EXPECTED_BUILD_RUNNER) { throw 'Wrong runner' }
      - uses: actions/checkout@v4
        with:
          clean: false
`;

function fixture(job = localJob, event = 'workflow_dispatch:') {
  return `on:\n  ${event}\njobs:\n  check:\n${job}`;
}

test('repository workflows preserve the designated runner and storage policy', () => {
  assert.deepEqual(verifyWorkflowPolicy(), []);
});
test('allows guarded local jobs and repository reusable workflows', () => {
  assert.deepEqual(workflowPolicyErrors('fixture.yml', fixture()), []);
  assert.deepEqual(workflowPolicyErrors('fixture.yml', fixture('    uses: ./.github/workflows/clawscribe-windows-release.yml\n')), []);
});

test('rejects every hosted runner and unresolved runner selection', () => {
  for (const runner of ['ubuntu-latest', 'windows-2022', 'macos-latest',
    '\n      - windows-latest', '\n      - self-hosted', '${{ matrix.os }}',
    '${{ inputs.runner }}', '[self-hosted, Windows, X64]']) {
    const source = fixture(localJob.replace('[self-hosted, Windows, X64, clawscribe]', runner));
    assert.ok(workflowPolicyErrors('fixture.yml', source).some(error => error.includes('runner labels')), runner);
  }
});

test('rejects remote cache and artifact storage, including artifact merge actions', () => {
  for (const source of ['uses: actions/upload-artifact@v4', 'uses: actions/upload-artifact/merge@v4',
    'uses: actions/cache@v4', 'uses: actions/cache/save@v4', 'cache: pnpm', 'cache: true']) {
    assert.ok(workflowPolicyErrors('fixture.yml', `${fixture()}${source}\n`).some(error => error.includes('storage')), source);
  }
});

test('allows only same-repository pull requests from trusted authors', () => {
  const guarded = `    if: ${trustedPullRequestCondition}\n${localJob}`;
  assert.deepEqual(workflowPolicyErrors('fixture.yml', fixture(guarded, 'pull_request:')), []);
  for (const job of [localJob,
    guarded.replace('github.event.pull_request.head.repo.full_name == github.repository', 'true'),
    guarded.replace('github.event.pull_request.author_association', "'OWNER'")]) {
    assert.ok(workflowPolicyErrors('fixture.yml', fixture(job, 'pull_request:')).some(error => error.includes('exclude forks')));
  }
});

test('rejects privileged PR triggers and unguarded reusable PR jobs', () => {
  assert.ok(workflowPolicyErrors('fixture.yml', fixture(localJob, 'pull_request_target:')).some(error => error.includes('pull_request_target')));
  assert.ok(workflowPolicyErrors('fixture.yml', fixture('    uses: ./.github/workflows/clawscribe-windows-release.yml\n', 'pull_request:')).some(error => error.includes('guarded local job')));
});

test('checks the machine guard independently for every job', () => {
  for (const weakened of [localJob.replace('$env:COMPUTERNAME -ine', '$env:COMPUTERNAME -ieq'),
    localJob.replace('          EXPECTED_BUILD_RUNNER:', '          OTHER_BUILD_RUNNER:'),
    localJob.replace('          clean: false', '          clean: true')]) {
    const source = `${fixture()}  second:\n${weakened}`;
    assert.ok(workflowPolicyErrors('fixture.yml', source).some(error => error.includes('(second)')));
  }
});

test('rejects external reusable workflows and unsupported job layouts', () => {
  assert.ok(workflowPolicyErrors('fixture.yml', fixture('    uses: example/repository/.github/workflows/build.yml@main\n')).length);
  assert.ok(workflowPolicyErrors('fixture.yml', 'jobs: { check: { runs-on: ubuntu-latest } }\n').length);
});

test('rejects alternate trigger layouts instead of missing a pull-request event', () => {
  for (const trigger of ['on: [pull_request]', 'on: { pull_request: {} }', 'on:\n    pull_request:']) {
    assert.ok(workflowPolicyErrors('fixture.yml', `${trigger}\njobs:\n  check:\n${localJob}`).some(error => error.includes('trigger declarations')));
  }
});
