import { lstatSync, readFileSync, realpathSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

// This report contains only the fixed invented corpus and its actual first
// responses. Never use this transport for application logs or user content.
export function reportBatches(source, sha) {
  if (!/^[a-f0-9]{40}$/.test(sha) || Buffer.byteLength(source) > 720 * 1024) throw new Error('Invalid synthetic report boundary');
  const first = /^\s*```json\s*\n([\s\S]*?)\n```/.exec(source);
  const header = first && JSON.parse(first[1]);
  if (!header || header.label !== 'PUBLIC SYNTHETIC EVALUATION — actual first attempts; independent factual review pending'
      || header.build_sha !== sha || !/^[a-f0-9]{64}$/.test(header.helper_sha256)
      || !header.fixture || !header.model_catalog) throw new Error('Synthetic report provenance does not match');
  const batches = [];
  let part = '', bytes = 0;
  for (const character of source) {
    const length = Buffer.byteLength(character);
    if (bytes + length > 58000) { batches.push(part); part = ''; bytes = 0; }
    part += character; bytes += length;
  }
  if (part) batches.push(part);
  if (batches.length > 16) throw new Error('Too many synthetic report batches');
  return batches;
}

async function publish() {
  const env = process.env;
  if (env.GITHUB_EVENT_NAME !== 'workflow_dispatch' || env.GITHUB_REPOSITORY !== 'ch3573r/ClawScribe'
      || env.RUNNER_ENVIRONMENT !== 'self-hosted' || !env.EXPECTED_BUILD_RUNNER
      || env.RUNNER_NAME?.toLowerCase() !== env.EXPECTED_BUILD_RUNNER.toLowerCase()
      || env.COMPUTERNAME?.toLowerCase() !== env.EXPECTED_BUILD_RUNNER.toLowerCase()
      || !/^\d+$/.test(env.GITHUB_RUN_ID ?? '') || !/^\d+$/.test(env.GITHUB_RUN_ATTEMPT ?? '')
      || !env.GITHUB_TOKEN) throw new Error('Trusted manual evaluation context required');
  const root = realpathSync(env.RUNNER_TEMP);
  const report = path.join(root, `clawscribe-answer-qa-${env.GITHUB_RUN_ID}-${env.GITHUB_RUN_ATTEMPT}.md`);
  const stat = lstatSync(report);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 720 * 1024
      || path.dirname(realpathSync(report)).toLowerCase() !== root.toLowerCase()) throw new Error('Invalid synthetic report file');
  const batches = reportBatches(readFileSync(report, 'utf8'), env.BUILD_REF);
  for (const [index, text] of batches.entries()) {
    const response = await fetch(`https://api.github.com/repos/${env.GITHUB_REPOSITORY}/check-runs`, {
      method: 'POST', signal: AbortSignal.timeout(30000),
      headers: {Accept:'application/vnd.github+json',Authorization:`Bearer ${env.GITHUB_TOKEN}`,'X-GitHub-Api-Version':'2022-11-28','Content-Type':'application/json'},
      body: JSON.stringify({name:`Synthetic answer review ${index + 1}/${batches.length}`,head_sha:env.BUILD_REF,
        status:'completed',conclusion:'neutral',
        details_url:`https://github.com/${env.GITHUB_REPOSITORY}/actions/runs/${env.GITHUB_RUN_ID}`,
        external_id:`answer-qa-${env.GITHUB_RUN_ID}-${env.GITHUB_RUN_ATTEMPT}-${index + 1}`,
        output:{title:`Public synthetic answer evaluation, batch ${index + 1}/${batches.length}`,
          summary:'Actual first attempts from the fixed invented corpus. Independent factual review is pending. Concatenate batch text in order to recover the complete bounded report; neutral does not assert acceptance.',text}}),
    });
    if (!response.ok) throw new Error(`Synthetic Check Run publication failed (${response.status})`);
    const result = await response.json();
    console.log(`Synthetic review check ${result.id}: ${result.html_url}`);
  }
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  publish().catch(() => { console.error('Synthetic evaluation report publication failed; inspect job status and permissions.'); process.exitCode = 1; });
}
