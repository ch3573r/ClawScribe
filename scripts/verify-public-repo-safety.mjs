#!/usr/bin/env node

import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const contentRules = [
  {
    name: 'personal Windows user-profile path',
    regex: /\b[A-Za-z]:(?:[\\/]{1,2})Users(?:[\\/]{1,2})(?!<username>(?:[\\/]{1,2})|%USERNAME%(?:[\\/]{1,2})|\$env:USERNAME(?:[\\/]{1,2}))[^\\/\r\n"'<>|]+(?:[\\/]{1,2})/gi,
  },
  {
    name: 'personal Windows workspace path',
    regex: /\b[A-Za-z]:(?:[\\/]{1,2})(?:AI|Code|Projects|Repos|Repositories|Documents|Desktop|Downloads)(?:[\\/]{1,2})[^\r\n"'<>|]*/gi,
  },
  {
    name: 'personal macOS home path',
    regex: /\/Users\/(?!<username>\/|Shared\/)[^/\s"'<>]+\//g,
  },
  {
    name: 'personal Linux home path',
    regex: /\/home\/(?!<username>\/)[^/\s"'<>]+\//g,
  },
  {
    name: 'private-key block',
    regex: /-----BEGIN (?:RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----/g,
  },
  {
    name: 'OpenAI-style API key',
    regex: /\bsk-(?:proj-)?[A-Za-z0-9_-]{20,}\b/g,
  },
  {
    name: 'GitHub access token',
    regex: /\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})\b/g,
  },
  {
    name: 'AWS access-key ID',
    regex: /\b(?:AKIA|ASIA)[0-9A-Z]{16}\b/g,
  },
  {
    name: 'Google API key',
    regex: /\bAIza[0-9A-Za-z_-]{30,}\b/g,
  },
  {
    name: 'Slack access token',
    regex: /\bxox[baprs]-[0-9A-Za-z-]{10,}\b/g,
  },
];

const credentialAssignment =
  /\b(?:user[_-]?name|password|passwd|api[_-]?key|client[_-]?secret|access[_-]?token|refresh[_-]?token)\b\s*[:=]\s*["']([^"']+)["']/gi;

const placeholderValue = /^(?:<[^>]+>|\$\{[^}]+\}|\$env:[A-Z0-9_]+|%[A-Z0-9_]+%|(?:your|replace|example|dummy|test|fake|redacted|not-a-real|change-?me)[-_].*|(?:test|testuser|dummy|example|exampleuser|fake|redacted|user|username|password|secret))$/i;

function lineNumberAt(text, index) {
  let line = 1;
  for (let cursor = 0; cursor < index; cursor += 1) {
    if (text.charCodeAt(cursor) === 10) line += 1;
  }
  return line;
}

function scanText(file, text) {
  const findings = [];
  for (const rule of contentRules) {
    rule.regex.lastIndex = 0;
    for (const match of text.matchAll(rule.regex)) {
      findings.push({ file, line: lineNumberAt(text, match.index), rule: rule.name });
    }
  }

  credentialAssignment.lastIndex = 0;
  for (const match of text.matchAll(credentialAssignment)) {
    if (!placeholderValue.test(match[1].trim())) {
      findings.push({
        file,
        line: lineNumberAt(text, match.index),
        rule: 'literal credential assignment',
      });
    }
  }
  return findings;
}

function selfTest() {
  const privateWindowsPath = ['C:', '\\Users\\', 'actual-user', '\\project'].join('');
  const escapedWindowsPath = ['C:', '\\\\Users\\\\', 'actual-user', '\\\\project'].join('');
  const forwardSlashWindowsPath = ['C:', '/Users/', 'actual-user', '/project'].join('');
  const privateWorkspace = ['D:', '\\Projects\\', 'private-repo'].join('');
  const apiKey = ['sk-', 'a'.repeat(30)].join('');
  const passwordAssignment = ['password = "', 'real-value-123', '"'].join('');

  assert(scanText('sample.txt', privateWindowsPath).some((item) => item.rule.includes('user-profile')));
  assert(scanText('sample.txt', escapedWindowsPath).some((item) => item.rule.includes('user-profile')));
  assert(scanText('sample.txt', forwardSlashWindowsPath).some((item) => item.rule.includes('user-profile')));
  assert(scanText('sample.txt', privateWorkspace).some((item) => item.rule.includes('workspace')));
  assert(scanText('sample.txt', apiKey).some((item) => item.rule.includes('API key')));
  assert(scanText('sample.txt', passwordAssignment).some((item) => item.rule.includes('credential')));
  assert.equal(scanText('sample.txt', String.raw`C:\Users\<username>\project`).length, 0);
  assert.equal(scanText('sample.txt', 'api_key = "${OPENAI_API_KEY}"').length, 0);
}

function repositoryFiles(repoRoot) {
  const output = execFileSync(
    'git',
    ['ls-files', '--cached', '--others', '--exclude-standard', '-z'],
    {
    cwd: repoRoot,
    encoding: 'buffer',
    },
  );
  return output
    .toString('utf8')
    .split('\0')
    .filter(Boolean);
}

function main() {
  selfTest();
  const repoRoot = execFileSync('git', ['rev-parse', '--show-toplevel'], {
    encoding: 'utf8',
  }).trim();
  const findings = [];

  const files = repositoryFiles(repoRoot);
  for (const file of files) {
    const absolutePath = resolve(repoRoot, file);
    if (!existsSync(absolutePath)) continue;
    const bytes = readFileSync(absolutePath);
    if (bytes.includes(0)) continue;
    findings.push(...scanText(file, bytes.toString('utf8')));
  }

  if (findings.length > 0) {
    console.error('Public-repository safety check failed:');
    for (const finding of findings) {
      console.error(`- ${finding.file}:${finding.line}: ${finding.rule}`);
    }
    console.error('Replace private values with environment variables or explicit placeholders.');
    process.exit(1);
  }

  console.log(`Public-repository safety check passed for ${files.length} tracked or unignored files.`);
}

main();
