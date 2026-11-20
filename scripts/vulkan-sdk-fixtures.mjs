// Helpers for the Vulkan SDK fixture tests (not a test file itself).
import assert from 'node:assert/strict';
import { copyFileSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import os from 'node:os';
import path from 'node:path';

export const scratch = () => mkdtempSync(path.join(os.tmpdir(), 'clawscribe-vulkan-test-'));

// A fake SDK: only the files the detection looks at.
export function makeSdk(folder, { major = 1, minor = 4, patch = 309, library = true, bin = true, header = true, tool = false } = {}) {
  mkdirSync(path.join(folder, 'Include', 'vulkan'), { recursive: true });
  if (header) {
    writeFileSync(path.join(folder, 'Include', 'vulkan', 'vulkan_core.h'), [
      '#define VK_MAKE_API_VERSION(variant, major, minor, patch) 0',
      `#define VK_HEADER_VERSION ${patch}`,
      `#define VK_HEADER_VERSION_COMPLETE VK_MAKE_API_VERSION(0, ${major}, ${minor}, VK_HEADER_VERSION)`,
      '',
    ].join('\n'));
  } else {
    writeFileSync(path.join(folder, 'Include', 'vulkan', 'vulkan_core.h'), '// no version here\n');
  }
  if (library !== false) {
    mkdirSync(path.join(folder, 'Lib'), { recursive: true });
    writeFileSync(path.join(folder, 'Lib', 'vulkan-1.lib'), library === 'empty' ? '' : 'library');
  }
  if (bin) mkdirSync(path.join(folder, 'Bin'), { recursive: true });
  if (tool && bin) {
    // A real executable whose file version is not the SDK's.
    copyFileSync(path.join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'whoami.exe'), path.join(folder, 'Bin', 'vulkaninfoSDK.exe'));
  }
  return folder;
}

// Runs PowerShell with values passed through the environment, never interpolated.
export function pwsh(command, env = {}) {
  const result = spawnSync('pwsh', ['-NoProfile', '-NonInteractive', '-Command', command], {
    encoding: 'utf8',
    env: { ...process.env, ...env },
  });
  assert.ok(!result.error, `pwsh must be available: ${result.error}`);
  return result;
}
