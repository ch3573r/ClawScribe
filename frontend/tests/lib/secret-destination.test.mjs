import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
const { needsHttpOptIn } = loadTsModule('src/lib/secretDestination.ts');
test('HTTP opt-in appears only for private network destinations', () => {
  for (const url of ['http://openclaw.local:8765', 'http://gateway', 'http://model.lan', 'http://wiki.internal', 'http://server.home.arpa', 'http://[fd00::1]']) {
    assert.equal(needsHttpOptIn(url), true, url);
  }
  for (const url of ['https://openclaw.local', 'http://public.example.com', 'http://127.0.0.1', 'http://localhost', 'http://[::1]', 'invalid']) {
    assert.equal(needsHttpOptIn(url), false, url);
  }
});
