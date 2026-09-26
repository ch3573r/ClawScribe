import { existsSync, readdirSync, readFileSync, rmSync } from 'node:fs';
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

function whisperCaches(profileDirectory) {
  const build = join(profileDirectory, 'build');
  return existsSync(build) ? readdirSync(build, { withFileTypes: true })
    .filter(entry => entry.isDirectory() && entry.name.startsWith('whisper-rs-sys-'))
    .map(entry => ({ name: entry.name, cache: join(build, entry.name, 'out', 'build', 'CMakeCache.txt') }))
    .filter(({ cache }) => existsSync(cache)) : [];
}

export function verifyProfile(profileDirectory) {
  const caches = whisperCaches(profileDirectory);
  if (!caches.length) throw new Error('No Whisper CMake cache found; refusing to bundle unverified native code.');
  for (const { name, cache } of caches) {
    try { verifyCache(readFileSync(cache, 'utf8')); } catch (error) { throw new Error(`${name}: ${error.message}`); }
  }
  return caches.length;
}

// A persistent runner keeps caches configured before the portable profile.
// Removing the build output and its fingerprint makes Cargo rerun the build
// script if that unit is needed again; nothing incompatible is reused.
export function removeStaleCaches(profileDirectory) {
  const removed = [];
  for (const { name, cache } of whisperCaches(profileDirectory)) {
    try { verifyCache(readFileSync(cache, 'utf8')); } catch {
      for (const directory of ['build', '.fingerprint']) {
        rmSync(join(profileDirectory, directory, name), { recursive: true, force: true });
      }
      removed.push(name);
    }
  }
  return removed;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const removeStale = process.argv.includes('--remove-stale');
    let profile = process.argv.slice(2).find(argument => argument !== '--remove-stale');
    if (!profile) {
      const frontend = resolve(dirname(fileURLToPath(import.meta.url)), '..');
      const metadata = JSON.parse(execFileSync('cargo', ['metadata', '--locked', '--no-deps', '--format-version', '1',
        '--manifest-path', join(frontend, 'src-tauri', 'Cargo.toml')], { encoding: 'utf8' }));
      profile = join(metadata.target_directory, 'release');
    }
    if (removeStale) {
      const removed = removeStaleCaches(profile);
      console.log(removed.length ? `Removed incompatible Whisper build cache(s): ${removed.join(', ')}` : 'No incompatible Whisper build caches found.');
    } else {
      console.log(`Verified ${verifyProfile(profile)} Whisper build cache(s) for Windows CPU portability.`);
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
