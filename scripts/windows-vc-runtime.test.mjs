import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
const audit = join(root, 'frontend/scripts/verify-packaged-vc-runtime.mjs');
const stage = join(root, 'frontend/scripts/stage-vc-runtime.ps1');
const config = join(root, 'frontend/src-tauri/tauri.windows-release.conf.json');
const crt = ['msvcp140.dll', 'msvcp140_1.dll', 'vcruntime140.dll', 'vcruntime140_1.dll', 'vcomp140.dll'];

function workspace(t) {
  const folder = mkdtempSync(join(tmpdir(), 'clawscribe-vc-test-'));
  t.after(() => rmSync(folder, { recursive: true, force: true }));
  return folder;
}

// A PE32+ image with one section and independently placed regular/delay import tables.
function image(regular = '', delayed = '', dll = true) {
  const file = Buffer.alloc(0x400);
  file.write('MZ');
  file.writeUInt32LE(0x80, 0x3c);
  file.write('PE\0\0', 0x80);
  file.writeUInt16LE(0x8664, 0x84);
  file.writeUInt16LE(1, 0x86);
  file.writeUInt16LE(0xf0, 0x94);
  file.writeUInt16LE(dll ? 0x2002 : 0x0002, 0x96);
  file.writeUInt16LE(0x20b, 0x98);
  file.writeUInt32LE(16, 0x104);
  file.writeUInt32LE(0x200, 0x190);
  file.writeUInt32LE(0x1000, 0x194);
  file.writeUInt32LE(0x200, 0x198);
  file.writeUInt32LE(0x200, 0x19c);
  if (regular) {
    file.writeUInt32LE(0x1000, 0x110);
    file.writeUInt32LE(40, 0x114);
    file.writeUInt32LE(0x1080, 0x20c);
    file.write(regular, 0x280);
  }
  if (delayed) {
    file.writeUInt32LE(0x1040, 0x170);
    file.writeUInt32LE(64, 0x174);
    file.writeUInt32LE(1, 0x240);
    file.writeUInt32LE(0x10c0, 0x244);
    file.write(delayed, 0x2c0);
  }
  return file;
}

function auditPackage(folder) {
  return spawnSync(process.execPath, [audit, folder, folder], { encoding: 'utf8' });
}

test('audits runtime dependencies of every packaged PE, including delay-loaded imports', t => {
  const folder = workspace(t);
  for (const name of crt) writeFileSync(join(folder, name), image());
  writeFileSync(join(folder, 'clawscribe.exe'), image('MSVCP140_1.DLL', 'api-ms-win-crt-runtime-l1-1-0.dll', false));
  const nested = join(folder, 'resources');
  mkdirSync(nested);
  writeFileSync(join(nested, 'native-module.node'), image('', 'MSVCP140_ATOMIC_WAIT.DLL'));

  const missing = auditPackage(folder);
  assert.equal(missing.status, 1);
  assert.match(missing.stderr, /native-module\.node.*msvcp140_atomic_wait\.dll/);

  writeFileSync(join(folder, 'msvcp140_atomic_wait.dll'), image('VCRUNTIME140.dll'));
  const complete = auditPackage(folder);
  assert.equal(complete.status, 0, complete.stderr);
});

test('requires every known VC++ DLL beside the main executable even if present deeper in the package', t => {
  const folder = workspace(t);
  for (const name of crt.filter(name => name !== 'vcomp140.dll')) writeFileSync(join(folder, name), image());
  const nested = join(folder, 'resources');
  mkdirSync(nested);
  writeFileSync(join(nested, 'vcomp140.dll'), image());

  const result = auditPackage(folder);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /vcomp140\.dll.*application folder/);
});

test('uses the MSI component directory and installed filename for cabinet entries', t => {
  const folder = workspace(t);
  const table = join(folder, 'file-table.json');
  const files = crt.map((name, index) => ({ key: 'PathFile_' + index, name, directory: 'INSTALLDIR' }));
  for (const { key } of files) writeFileSync(join(folder, key), image());
  files.push({ key: 'Path', name: 'clawscribe.exe', directory: 'INSTALLDIR' });
  writeFileSync(join(folder, 'Path'), image('MSVCP140.dll', '', false));
  const check = () => spawnSync(process.execPath, [audit, folder, 'INSTALLDIR', table], { encoding: 'utf8' });

  writeFileSync(table, JSON.stringify(files));
  const complete = check();
  assert.equal(complete.status, 0, complete.stderr);

  files[0].directory = 'RESOURCES';
  writeFileSync(table, JSON.stringify(files));
  const misplaced = check();
  assert.equal(misplaced.status, 1);
  assert.match(misplaced.stderr, /clawscribe\.exe requires msvcp140\.dll/);
});

test('the release bundle maps each runtime DLL directly beside the executables', () => {
  const { bundle } = JSON.parse(readFileSync(config, 'utf8'));
  assert.deepEqual(bundle.resources, Object.fromEntries(crt.map(name => [`binaries/vc-runtime/${name}`, name])));
});

test('staging fails before copying when the selected MSVC redistribution directory lacks OpenMP', {
  skip: process.platform !== 'win32',
}, t => {
  const folder = workspace(t);
  const redist = join(folder, 'x64', 'Microsoft.VC143.CRT');
  mkdirSync(redist, { recursive: true });
  for (const name of crt.filter(name => name !== 'vcomp140.dll')) writeFileSync(join(redist, name), image());
  const result = spawnSync('pwsh', ['-NoProfile', '-File', stage, '-TauriRoot', folder], {
    encoding: 'utf8', env: { ...process.env, VCToolsRedistDir: folder },
  });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Missing.*vcomp140\.dll/);
  assert.equal(existsSync(join(folder, 'binaries')), false);
});

test('staging rejects unsigned files in a redistributable directory', {
  skip: process.platform !== 'win32',
}, t => {
  const folder = workspace(t);
  for (const name of crt) {
    const component = name === 'vcomp140.dll' ? 'OpenMP' : 'CRT';
    const redist = join(folder, 'x64', 'Microsoft.VC143.' + component);
    mkdirSync(redist, { recursive: true });
    writeFileSync(join(redist, name), image());
  }
  const result = spawnSync('pwsh', ['-NoProfile', '-File', stage, '-TauriRoot', folder], {
    encoding: 'utf8', env: { ...process.env, VCToolsRedistDir: folder },
  });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /valid Microsoft Authenticode signature/);
  assert.equal(existsSync(join(folder, 'binaries')), false);
});
