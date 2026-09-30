// Run only on the designated Windows build runner, after staging the sidecar.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';

const tauriRoot = fileURLToPath(new URL('../src-tauri/', import.meta.url));
// Windows PowerShell 5's UTF-8 staging output includes a BOM.
const manifestText = await readFile(path.join(tauriRoot, 'binaries/codex-app-server-runtime.json'), 'utf8');
const manifest = JSON.parse(manifestText.replace(/^\uFEFF/, ''));
const executable = path.resolve(tauriRoot, manifest.entrypoint);
assert.equal(createHash('sha256').update(await readFile(executable)).digest('hex'), manifest.runtime_sha256);
const scratch = await mkdtemp(path.join(tmpdir(), 'clawscribe-codex-smoke-'));
assert.equal(path.dirname(scratch), path.resolve(tmpdir()));
let child;
let closed;
let lines;
const pending = new Map();
let nextId = 1;

function failPending() {
  for (const { reject } of pending.values()) reject(new Error('Codex runtime smoke transport closed'));
}

function request(method, params) {
  const id = nextId++;
  let timer;
  return new Promise((resolve, reject) => {
    timer = setTimeout(() => reject(new Error(`Codex runtime smoke timed out: ${method}`)), 30_000);
    pending.set(id, { resolve, reject });
    child.stdin.write(`${JSON.stringify({ id, method, params })}\n`);
  }).finally(() => {
    clearTimeout(timer);
    pending.delete(id);
  });
}

try {
  // A fresh keyring-only profile cannot reuse the user's normal Codex sign-in.
  await writeFile(path.join(scratch, 'config.toml'), 'cli_auth_credentials_store = "keyring"\n');
  const env = { CODEX_HOME: scratch, CODEX_MANAGED_BY_CLAWSCRIBE: '1', CODEX_MANAGED_BY_NPM: '1' };
  for (const key of ['SystemRoot', 'WINDIR', 'TEMP', 'TMP', 'USERPROFILE', 'APPDATA', 'LOCALAPPDATA']) {
    if (process.env[key]) env[key] = process.env[key];
  }
  child = spawn(executable, ['app-server'], { cwd: scratch, env, windowsHide: true, stdio: ['pipe', 'pipe', 'ignore'] });
  closed = once(child, 'close');
  child.on('error', failPending);
  child.on('close', failPending);
  child.stdin.on('error', failPending);
  lines = createInterface({ input: child.stdout });
  lines.on('line', line => {
    let message;
    try { message = JSON.parse(line); } catch { failPending(); return; }
    const waiter = pending.get(message.id);
    if (!waiter) return;
    // Never print provider payloads, authentication data, or callback URLs.
    if (message.error) waiter.reject(new Error('Codex runtime smoke RPC failed'));
    else waiter.resolve(message.result);
  });
  await request('initialize', { clientInfo: { name: 'clawscribe', version: 'runtime-smoke' } });
  child.stdin.write(`${JSON.stringify({ method: 'initialized' })}\n`);
  const account = await request('account/read', {});
  assert.equal(account.account, null, 'Smoke profile must remain signed out');
  const models = [];
  let cursor = null;
  for (let page = 0; page < 20; page++) {
    const result = await request('model/list', { limit: 100, includeHidden: false, cursor });
    assert.ok(Array.isArray(result.data));
    models.push(...result.data);
    cursor = result.nextCursor;
    if (!cursor) break;
  }
  assert.ok(!cursor, 'Model pagination must terminate');
  const sol = models.find(model => model.id === 'gpt-6.1-sol');
  assert.ok(sol && !sol.hidden, 'GPT-6.1 Sol must be picker-visible');
  assert.ok(sol.supportedReasoningEfforts.some(effort => effort.reasoningEffort === sol.defaultReasoningEffort));
  await request('account/logout', {});
  assert.equal((await request('account/read', {})).account, null);
  console.log(`Codex ${manifest.runtime_version}: integrity, initialization, signed-out auth/logout, and GPT-6.1 Sol catalog passed (${models.length} models).`);
} finally {
  lines?.close();
  child?.kill();
  if (closed) await closed.catch(() => {});
  await rm(scratch, { recursive: true, force: true });
}
