import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
const { needsHttpOptIn, secretDestinationError } = loadTsModule('src/lib/secretDestination.ts');
test('HTTP opt-in appears only for private network destinations', () => {
  for (const url of ['http://openclaw.local:8765', 'http://gateway', 'http://model.lan', 'http://wiki.internal', 'http://server.home.arpa', 'http://[fd00::1]']) {
    assert.equal(needsHttpOptIn(url), true, url);
  }
  for (const url of ['https://openclaw.local', 'http://public.example.com', 'http://127.0.0.1', 'http://localhost', 'http://[::1]', 'invalid']) {
    assert.equal(needsHttpOptIn(url), false, url);
  }
});

test('Tailscale range and MagicDNS require opt-in without admitting public neighbours', () => {
  for (const url of ['http://100.64.0.0', 'http://100.127.255.255', 'http://host.example.ts.net']) {
    assert.equal(needsHttpOptIn(url), true, url);
    assert.ok(secretDestinationError(url, false), url);
    assert.equal(secretDestinationError(url, true), null, url);
  }
  for (const url of ['http://100.63.255.255', 'http://100.128.0.0', 'http://ts.net', 'http://host.ts.net.example.com']) {
    assert.equal(needsHttpOptIn(url), false, url);
    assert.match(secretDestinationError(url, true), /Public HTTP/, url);
  }
});

test('saved endpoints needing repair can be displayed with actionable warnings', () => {
  assert.match(secretDestinationError('http://public.example.com', true), /HTTPS/);
  assert.match(secretDestinationError('invalid', true), /valid endpoint/);
  assert.match(secretDestinationError('ftp://example.com', true), /HTTP or HTTPS/);
  assert.match(secretDestinationError('https://user@example.com', true), /without credentials/);
  assert.equal(secretDestinationError('https://public.example.com', false), null);
  assert.equal(secretDestinationError('http://127.0.0.2', false), null);
});
