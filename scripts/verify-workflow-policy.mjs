import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

export const trustedPullRequestCondition = "${{ github.event_name != 'pull_request' || (github.event.pull_request.head.repo.full_name == github.repository && contains(fromJSON('[\"OWNER\", \"MEMBER\", \"COLLABORATOR\"]'), github.event.pull_request.author_association)) }}";
const manualAnswerQaCondition = "${{ github.event_name == 'workflow_dispatch' && inputs.knowledge && inputs['answer-acceptance'] }}";
const localRunner = '[self-hosted, Windows, X64, clawscribe]';

// Workflows use block-style job declarations. Reject unknown runner syntax and
// external reusable workflows instead of assuming they use the local machine.
function workflowJobs(source) {
  const lines = source.split(/\r?\n/);
  const jobsStart = lines.indexOf('jobs:');
  if (jobsStart < 0) return [];
  const jobs = [];
  for (const line of lines.slice(jobsStart + 1)) {
    if (/^[^\s#]/.test(line)) break;
    const header = /^  ([\w-]+):\s*$/.exec(line);
    if (header) jobs.push({ name: header[1], lines: [] });
    else if (jobs.length) jobs.at(-1).lines.push(line);
  }
  return jobs.map(job => ({ name: job.name, source: job.lines.join('\n') }));
}

function workflowTriggers(source) {
  const lines = source.split(/\r?\n/);
  const triggerStart = lines.indexOf('on:');
  if (triggerStart < 0) return null;
  const triggers = [];
  for (const line of lines.slice(triggerStart + 1)) {
    if (/^[^\s#]/.test(line)) break;
    if (!line.trim() || /^\s*#/.test(line)) continue;
    const trigger = /^  (\w+):/.exec(line);
    if (trigger) triggers.push(trigger[1]);
    else if (!/^ {4}/.test(line)) return null;
  }
  return triggers.length ? triggers : null;
}

export function workflowPolicyErrors(name, source) {
  const errors = [];
  if (/uses:\s*(?:actions\/(?:upload-artifact|cache)(?:\/[^\s@]+)?|Swatinem\/rust-cache)@/i.test(source)
      || /^\s*cache:\s*(?:true|pnpm|npm|yarn)\s*$/im.test(source)) {
    errors.push(`${name}: Actions artifact/cache storage is disabled`);
  }
  if (/hosted-runner/.test(source)) errors.push(`${name}: hosted fallback is forbidden`);
  const runners = [...source.matchAll(/^[ \t]*runs-on:[ \t]*(.*)$/gm)];
  if (runners.some(match => match[1].trim() !== localRunner)) {
    errors.push(`${name}: all jobs require the designated local runner labels; runner expressions are forbidden`);
  }
  const triggers = workflowTriggers(source);
  if (!triggers) errors.push(`${name}: unsupported or missing trigger declarations`);
  const pullRequests = triggers?.includes('pull_request');
  if (triggers?.includes('pull_request_target')) {
    errors.push(`${name}: pull_request_target must not execute on the persistent runner`);
  }
  const jobs = workflowJobs(source);
  if (!jobs.length) errors.push(`${name}: unsupported or missing job declarations`);
  for (const job of jobs) {
    const label = `${name} (${job.name})`;
    const runner = /^    runs-on:[ \t]*(.*)$/m.exec(job.source);
    if (!runner) {
      if (!/^    uses: \.\/\.github\/workflows\/[\w-]+\.ya?ml\s*$/m.test(job.source)) {
        errors.push(`${label}: jobs must declare the local runner or use a repository workflow`);
      }
      if (pullRequests) errors.push(`${label}: pull requests must use a guarded local job`);
      continue;
    }
    if (pullRequests && ![trustedPullRequestCondition, manualAnswerQaCondition].includes(/^    if:[ \t]*(.*)$/m.exec(job.source)?.[1].trim())) {
      errors.push(`${label}: exclude forks and untrusted authors before scheduling a pull-request job`);
    }
    const checkout = job.source.indexOf('uses: actions/checkout@');
    const guard = job.source.indexOf('EXPECTED_BUILD_RUNNER: ${{ vars.CLAWSCRIBE_BUILD_RUNNER }}');
    if (guard < 0 || checkout < guard
        || !job.source.includes("$env:RUNNER_ENVIRONMENT -ne 'self-hosted'")
        || !job.source.includes('[string]::IsNullOrWhiteSpace($env:EXPECTED_BUILD_RUNNER)')
        || !job.source.includes('$env:RUNNER_NAME -ine $env:EXPECTED_BUILD_RUNNER')
        || !job.source.includes('$env:COMPUTERNAME -ine $env:EXPECTED_BUILD_RUNNER')) {
      errors.push(`${label}: verify the designated machine before checkout`);
    }
    if (!/uses: actions\/checkout@[^\n]+\n\s+with:\s*\n(?:[^\n]*\n)*?\s+clean: false/m.test(job.source)) {
      errors.push(`${label}: preserve local build caches on checkout`);
    }
  }
  return errors;
}

export function verifyWorkflowPolicy(root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')) {
  const directory = path.join(root, '.github', 'workflows');
  return readdirSync(directory).filter(name => /\.ya?ml$/.test(name))
    .flatMap(name => workflowPolicyErrors(name, readFileSync(path.join(directory, name), 'utf8')));
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const errors = verifyWorkflowPolicy();
  if (errors.length) {
    console.error(errors.join('\n'));
    process.exitCode = 1;
  } else console.log('Local Windows runner and Actions storage policy verified.');
}
