import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

export function verifyCache(contents) {
  for (const flag of ['GGML_NATIVE', 'GGML_AVX512', 'GGML_AVX512_VBMI', 'GGML_AVX512_VNNI', 'GGML_AVX512_BF16']) {
    if (!new RegExp(`^${flag}:BOOL=OFF\\r?$`, 'm').test(contents)) {
      throw new Error(`${flag} must be explicitly OFF. Rebuild with configure-windows-portability.ps1.`);
    }
  }
  const compilerFlags = contents.split(/\r?\n/).filter(line => /^CMAKE_C(?:XX)?_FLAGS[^:]*:/.test(line)).join('\n');
  if (/(?:-march=native|-mcpu=native|-mavx512|\/arch:AVX512)/i.test(compilerFlags)) {
    throw new Error('Native compiler flags exceed the portable Windows CPU baseline.');
  }
}

export function verifyProfile(profileDirectory) {
  const build = join(profileDirectory, 'build');
  const caches = existsSync(build) ? readdirSync(build, { withFileTypes: true })
    .filter(entry => entry.isDirectory() && entry.name.startsWith('whisper-rs-sys-'))
    .map(entry => join(build, entry.name, 'out', 'build', 'CMakeCache.txt')).filter(existsSync) : [];
  if (!caches.length) throw new Error('No Whisper CMake cache found; refusing to bundle unverified native code.');
  for (const cache of caches) verifyCache(readFileSync(cache, 'utf8'));
  return caches.length;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    let profile = process.argv[2];
    if (!profile) {
      const frontend = resolve(dirname(fileURLToPath(import.meta.url)), '..');
      const metadata = JSON.parse(execFileSync('cargo', ['metadata', '--locked', '--no-deps', '--format-version', '1',
        '--manifest-path', join(frontend, 'src-tauri', 'Cargo.toml')], { encoding: 'utf8' }));
      profile = join(metadata.target_directory, 'release');
    }
    console.log(`Verified ${verifyProfile(profile)} Whisper build cache(s) for Windows CPU portability.`);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
