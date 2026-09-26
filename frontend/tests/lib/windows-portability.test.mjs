import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { verifyCache, verifyProfile } from '../../scripts/verify-windows-portability.mjs';

const flags = ['GGML_NATIVE', 'GGML_AVX512', 'GGML_AVX512_VBMI', 'GGML_AVX512_VNNI', 'GGML_AVX512_BF16'];
const cache = flags.map(flag => `${flag}:BOOL=OFF`).join('\n');
test('portable CPU flags permit AVX2 but reject native and AVX512 code', () => {
  verifyCache(`${cache}\nGGML_AVX2:BOOL=ON\nCMAKE_CXX_FLAGS:STRING=/arch:AVX2`);
  for (const flag of flags) {
    assert.throws(() => verifyCache(cache.replace(`${flag}:BOOL=OFF`, `${flag}:BOOL=ON`)));
    assert.throws(() => verifyCache(cache.replace(`${flag}:BOOL=OFF`, '')));
  }
  for (const compilerFlag of ['-march=native', '-mavx512f', '/arch:AVX512']) {
    assert.throws(() => verifyCache(`${cache}\nCMAKE_CXX_FLAGS_RELEASE:STRING=${compilerFlag}`));
  }
});
test('bundling fails closed for missing or incompatible cached builds', () => {
  const root = mkdtempSync(join(tmpdir(), 'clawscribe-portability-'));
  try {
    assert.throws(() => verifyProfile(root), /No Whisper/);
    for (const name of ['good', 'stale']) {
      const directory = join(root, 'build', `whisper-rs-sys-${name}`, 'out', 'build');
      mkdirSync(directory, { recursive: true });
      writeFileSync(join(directory, 'CMakeCache.txt'), name === 'good' ? cache : cache.replace('GGML_NATIVE:BOOL=OFF', 'GGML_NATIVE:BOOL=ON'));
    }
    assert.throws(() => verifyProfile(root), /GGML_NATIVE/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('Windows setup preserves unrelated flags and rejects CPU overrides', { skip: process.platform !== 'win32' }, () => {
  const script = fileURLToPath(new URL('../../scripts/configure-windows-portability.ps1', import.meta.url));
  const quoted = `'${script.replaceAll("'", "''")}'`;
  const run = (flags, encoded = '') => spawnSync('powershell.exe', ['-NoProfile', '-Command',
    `try { . ${quoted}; . ${quoted}; Write-Output $env:RUSTFLAGS } catch { exit 1 }`], {
    encoding: 'utf8', env: { ...process.env, RUSTFLAGS: flags, CARGO_ENCODED_RUSTFLAGS: encoded },
  });
  const valid = run('-C debuginfo=1');
  assert.equal(valid.status, 0, valid.stderr);
  assert.equal(valid.stdout.trim(), '-C debuginfo=1 -C target-cpu=x86-64-v2');
  for (const flags of ['-C target-cpu=native', '-Ctarget-cpu=x86-64-v3', '-C target-feature=+avx512f']) {
    assert.notEqual(run(flags).status, 0);
  }
  assert.notEqual(run('', '-C\x1ftarget-cpu=native').status, 0);
});
