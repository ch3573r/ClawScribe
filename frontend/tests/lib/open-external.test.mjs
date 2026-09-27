import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import path from 'node:path';
import { loadTsModule } from './load-ts-module.mjs';

test('external links preserve queries and reject other schemes without invoking', async () => {
  const calls = [], errors = [];
  const { openExternal } = loadTsModule('src/lib/openExternal.ts', {
    '@tauri-apps/api/core': { invoke: async (...args) => { calls.push(args); } },
    sonner: { toast: { error: (...args) => errors.push(args) } },
  });
  for (const url of ['https://example.com/a?b=1&c=2', 'http://example.com/']) await openExternal(url);
  assert.equal(calls.length, 2);
  assert.equal(calls[0][0], 'open_external_url');
  assert.equal(calls[0][1].url, 'https://example.com/a?b=1&c=2');
  for (const url of ['file:///C:/Windows/System32/calc.exe', 'javascript:alert(1)', 'C:\\Windows\\System32\\calc.exe', 'https://x&calc', 'ms-settings:', '']) await openExternal(url);
  assert.equal(calls.length, 2);
  assert.equal(errors.length, 6);
});

test('external link call sites use the shared helper', () => {
  const visit = folder => fs.readdirSync(folder, { withFileTypes: true }).flatMap(entry => {
    const file = path.join(folder, entry.name);
    return entry.isDirectory() ? visit(file) : [file];
  });
  for (const file of visit('src').filter(file => /\.[jt]sx?$/.test(file))) {
    const source = fs.readFileSync(file, 'utf8');
    assert.doesNotMatch(source, /window\.open\s*\(/, file);
    for (const anchor of source.matchAll(/<a\b[^>]*>/gs)) {
      if (/target\s*=\s*["']_blank/.test(anchor[0])) assert.match(anchor[0], /openExternal\(/, file);
    }
  }
});
