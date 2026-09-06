import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

// Deliberately fail closed for new runner expressions: a policy change needs
// review before native code is allowed onto another machine.
export function workflowPolicyErrors(name, source) {
  const errors = [];
  if (/uses:\s*(?:actions\/(?:upload-artifact|cache)|Swatinem\/rust-cache)@/i.test(source)
      || /^\s*cache:\s*(?:true|pnpm|npm|yarn)\s*$/im.test(source)) {
    errors.push(`${name}: Actions artifact/cache storage is disabled`);
  }
  if (/hosted-runner/.test(source)) errors.push(`${name}: hosted fallback is forbidden`);
  const native = /runs-on:.*(?:self-hosted|windows)/i.test(source);
  if (native) {
    const runners = [...source.matchAll(/^\s*runs-on:\s*(.+)$/gm)].map(match => match[1].trim());
    if (runners.some(runner => runner !== '[self-hosted, Windows, X64, clawscribe]')) {
      errors.push(`${name}: native jobs require the designated local runner labels`);
    }
    if (/^\s*pull_request(?:_target)?:/m.test(source)) {
      errors.push(`${name}: untrusted pull requests must not execute on the persistent runner`);
    }
    const checkout = source.indexOf('uses: actions/checkout@');
    const guard = source.indexOf('EXPECTED_BUILD_RUNNER: ${{ vars.CLAWSCRIBE_BUILD_RUNNER }}');
    if (guard < 0 || checkout < guard
        || !source.includes("$env:RUNNER_ENVIRONMENT -ne 'self-hosted'")
        || !source.includes('$env:RUNNER_NAME -ine $env:EXPECTED_BUILD_RUNNER')
        || !source.includes('$env:COMPUTERNAME -ine $env:EXPECTED_BUILD_RUNNER')) {
      errors.push(`${name}: verify the designated machine before checkout`);
    }
    if (!/uses: actions\/checkout@[^\n]+\r?\n\s+with:\s*\r?\n(?:[^\n]*\r?\n)*?\s+clean: false/m.test(source)) {
      errors.push(`${name}: preserve local build caches on checkout`);
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
