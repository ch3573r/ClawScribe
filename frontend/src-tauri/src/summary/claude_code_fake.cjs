// Synthetic Claude Code protocol fixture. Never used by the app.
const fs = require('node:fs');
const path = require('node:path');
const mode = 'FIXTURE_MODE';
const args = process.argv.slice(2);
if (args[0] === '--version') {
  console.log(mode === 'bad-version' ? 'invalid' : '2.1.0 (Claude Code)');
} else if (args[0] === 'auth') {
  console.log(JSON.stringify(mode === 'unknown-auth' ? {} : { loggedIn: mode !== 'signed-out', subscriptionType: 'max', email: 'private@example.invalid' }));
  if (mode === 'signed-out') process.exitCode = 1;
} else {
  const file = path.join(__dirname, 'observed.json');
  const attempts = fs.existsSync(file) ? JSON.parse(fs.readFileSync(file)).attempts + 1 : 1;
  const initialFiles = fs.readdirSync(process.cwd());
  const systemIndex = args.indexOf('--system-prompt-file');
  const systemBytes = systemIndex < 0 ? null : Array.from(fs.readFileSync(args[systemIndex + 1]));
  let input = '';
  process.stdin.setEncoding('utf8');
  process.stdin.on('data', data => input += data);
  process.stdin.on('end', () => {
    let descendantPid = null;
    if (mode === 'hold-pipes') {
      const { spawn } = require('node:child_process');
      const descendant = spawn(process.execPath, ['-e', `
        const fs = require('node:fs');
        const cwd = process.cwd();
        const timer = setInterval(() => { if (!fs.existsSync(cwd)) process.exit(0); }, 100);
        setTimeout(() => { clearInterval(timer); process.exit(0); }, 15000);
      `], { stdio: ['ignore', 'inherit', 'inherit'], windowsHide: true });
      descendantPid = descendant.pid;
      descendant.unref();
    }
    fs.writeFileSync(file, JSON.stringify({ attempts, args, input, systemBytes, descendantPid, cwd: process.cwd(), initialFiles,
      pid: process.pid, parentPid: process.platform === 'win32' ? process.ppid : process.pid,
      preservedTestVariable: process.env.CLAWSCRIBE_TEST_PRESERVED ?? null,
      leakedStoredKey: Object.values(process.env).includes('synthetic-stored-key'),
      secretKeys: Object.keys(process.env).filter(key => /^(ANTHROPIC_(API_KEY|AUTH_TOKEN|BASE_URL)|CLAUDECODE|CLAUDE_CODE_.*)$/i.test(key)) }));
    if (mode === 'hang') { setInterval(() => {}, 1000); return; }
    if (mode === 'always-unknown' || (mode === 'old-cli' && args.includes('--no-session-persistence'))) {
      console.error("error: unknown option '--no-session-persistence'"); process.exitCode = 1; return;
    }
    const result = { type: 'result', subtype: 'success', result: 'safe answer' };
    if (mode === 'error') Object.assign(result, { is_error: true, subtype: 'error', result: 'usage limit reached' });
    if (mode === 'expired') {
      Object.assign(result, { is_error: true, result: 'Failed to authenticate: OAuth session expired' });
      process.exitCode = 1;
    }
    if (mode === 'hold-pipes') {
      process.stderr.write('synthetic stderr\n');
      process.stdout.write(JSON.stringify(result) + '\n', () => process.exit(0));
      return;
    }
    console.log(JSON.stringify(mode === 'array' ? [{ type: 'assistant', result: 'ignore' }, result] : result));
  });
}
