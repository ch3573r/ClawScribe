import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

const secret = loadTsModule('src/lib/secret-key.ts');
// Modules run in their own realm, so compare structure through JSON.
const same = (actual, expected, message) => assert.equal(JSON.stringify(actual), JSON.stringify(expected), message);

// Fake values only.
const FAKE_KEY = 'fake-api-key-value-7777';

test('a status is read tolerantly and anything unexpected means no secret', () => {
  same(secret.readSecretStatus({ stored: true, hint: '7777' }), { stored: true, hint: '7777' });
  same(secret.readSecretStatus({ stored: true, hint: null }), { stored: true, hint: null });
  same(secret.readSecretStatus({ stored: true, hint: '' }), { stored: true, hint: null });
  for (const unexpected of [undefined, null, 'text', 5, [], {}, { stored: 'yes' }, { stored: false, hint: '7777' }]) {
    same(secret.readSecretStatus(unexpected), { stored: false, hint: null }, JSON.stringify(unexpected));
  }
});

test('a stored secret is described by its hint, never by its value', () => {
  assert.equal(secret.describeSecret({ stored: true, hint: '7777' }), 'Saved, ends in 7777');
  assert.equal(secret.describeSecret({ stored: true, hint: null }), 'Saved');
  assert.equal(secret.describeSecret({ stored: false, hint: null }), 'Not set');
  assert.equal(secret.describeSecret(null), 'Not set');
});

test('an untouched key field keeps the stored key: nothing is sent', () => {
  for (const untouched of [undefined, null, '', '   ', '\t\n']) {
    assert.equal(secret.newKeyPayload(untouched), null, JSON.stringify(untouched));
  }
});

test('a replaced key is sent trimmed, and only what was typed is ever sent', () => {
  assert.equal(secret.newKeyPayload(`  ${FAKE_KEY}  `), FAKE_KEY);
  // A hint is not a key: nothing in the status can become a payload.
  const status = { stored: true, hint: '7777' };
  assert.equal(secret.newKeyPayload(undefined), null);
  assert.notEqual(secret.newKeyPayload(status.hint), null, 'typed text is sent as typed, including text that looks like a hint');
});

test('a request can be made with a typed key or a stored one', () => {
  const stored = { stored: true, hint: '7777' };
  const none = { stored: false, hint: null };
  assert.equal(secret.hasUsableKey('', stored), true);
  assert.equal(secret.hasUsableKey(FAKE_KEY, none), true);
  assert.equal(secret.hasUsableKey('  ', none), false);
  assert.equal(secret.hasUsableKey(null, undefined), false);
  assert.equal(secret.hasUsableKey(null, null), false);
});

test('removing the key of the provider in use asks first; any other provider does not', () => {
  assert.equal(secret.removalNeedsConfirmation('openai', 'openai'), true);
  assert.equal(secret.removalNeedsConfirmation('claude', 'openai'), false);
  assert.equal(secret.removalNeedsConfirmation('claude', null), false);
  assert.equal(secret.removalNeedsConfirmation('claude', undefined), false);
});

test('anything read from the core loses secret fields, wherever they came from', () => {
  const raw = {
    provider: 'openai', model: 'm', apiKey: FAKE_KEY, api_key: FAKE_KEY, bearerToken: FAKE_KEY, bearer_token: FAKE_KEY,
    pat: FAKE_KEY, token: FAKE_KEY, accessToken: FAKE_KEY, refreshToken: FAKE_KEY, password: FAKE_KEY, customOpenAIApiKey: FAKE_KEY,
    apiKeyStatus: { stored: true, hint: '7777' },
  };
  const clean = secret.withoutSecrets(raw);
  assert.equal(JSON.stringify(clean).includes(FAKE_KEY), false);
  assert.equal(clean.provider, 'openai');
  same(clean.apiKeyStatus, { stored: true, hint: '7777' });
  // The original is not modified, and non-objects pass through.
  assert.equal(raw.apiKey, FAKE_KEY);
  assert.equal(secret.withoutSecrets(null), null);
  assert.equal(secret.withoutSecrets('text'), 'text');
});

test('no service function returns a secret: every read goes through the status mapping', async () => {
  const calls = [];
  const invoke = async (command, args) => {
    calls.push([command, args]);
    // Even if the core mistakenly returned a key, the service drops it.
    if (command === 'api_get_model_config') return { provider: 'openai', model: 'm', whisperModel: 'w', apiKey: FAKE_KEY, apiKeyStatus: { stored: true, hint: '7777' } };
    if (command === 'api_get_custom_openai_config') return { endpoint: 'https://gateway.example.com/v1', model: 'm', apiKey: FAKE_KEY, apiKeyStatus: { stored: true, hint: '7777' } };
    if (command === 'api_get_api_key_status' || command === 'api_get_transcript_api_key_status') return { stored: true, hint: '7777', apiKey: FAKE_KEY };
    return undefined;
  };
  const { configService } = loadTsModule('src/services/configService.ts', {
    '@tauri-apps/api/core': { invoke },
    '@/components/TranscriptSettings': {},
  });
  const results = [
    await configService.getModelConfig(),
    await configService.getCustomOpenAIConfig(),
    await configService.getApiKeyStatus('openai'),
    await configService.getTranscriptApiKeyStatus('cloud-whisper'),
  ];
  for (const result of results) assert.equal(JSON.stringify(result).includes(FAKE_KEY), false, JSON.stringify(result));
  same(results[2], { stored: true, hint: '7777' });
  same(results[0].apiKeyStatus, { stored: true, hint: '7777' });
  // The removed commands that returned a key are never called.
  assert.equal(calls.some(([command]) => command === 'api_get_api_key' || command === 'api_get_transcript_api_key'), false);
});

test('saving the custom provider without touching the key sends no key, and removing uses its own command', async () => {
  const calls = [];
  const { configService } = loadTsModule('src/services/configService.ts', {
    '@tauri-apps/api/core': { invoke: async (command, args) => { calls.push([command, args]); } },
    '@/components/TranscriptSettings': {},
  });
  await configService.saveCustomOpenAIConfig({ endpoint: 'https://gateway.example.com/v1', model: 'm', maxTokens: null, temperature: null, topP: null });
  assert.equal(calls[0][0], 'api_save_custom_openai_config');
  assert.equal(calls[0][1].apiKey, null, 'untouched: nothing is sent, so the core keeps the stored key');
  await configService.clearCustomOpenAIApiKey();
  await configService.clearApiKey('openai');
  await configService.clearTranscriptApiKey('cloud-whisper');
  same(calls.slice(1).map(([command]) => command), ['api_clear_custom_openai_api_key', 'api_clear_api_key', 'api_clear_transcript_api_key']);
  same(calls[2][1], { provider: 'openai' });
});

test('a typed key shorter than eight characters is a problem, and the four-character hint can never replace a key', () => {
  assert.equal(secret.keyProblem(''), null, 'an empty field keeps the stored key');
  assert.equal(secret.keyProblem(null), null);
  assert.equal(secret.keyProblem('   '), null);
  assert.equal(secret.keyProblem('7777'), secret.KEY_TOO_SHORT, 'the hint typed back is refused');
  assert.equal(secret.keyProblem('1234567'), secret.KEY_TOO_SHORT);
  assert.equal(secret.keyProblem('  1234567  '), secret.KEY_TOO_SHORT, 'whitespace does not count');
  assert.equal(secret.keyProblem('12345678'), null);
  assert.equal(secret.keyProblem(FAKE_KEY), null);
  assert.equal(secret.MIN_KEY_CHARS, 8);
});

test('an address is the same ignoring case and a trailing slash, but never another host, port, path or downgraded scheme', () => {
  assert.equal(secret.keepsAddress('https://gateway.example.com/v1', 'https://gateway.example.com/v1/'), true);
  assert.equal(secret.keepsAddress(' HTTPS://Gateway.Example.com:443/V1 ', 'https://gateway.example.com/v1'), true);
  assert.equal(secret.keepsAddress('https://gateway.example.com/v1', 'https://other.example.com/v1'), false);
  assert.equal(secret.keepsAddress('https://gateway.example.com/v1', 'https://gateway.example.com:8443/v1'), false);
  assert.equal(secret.keepsAddress('https://gateway.example.com/v1', 'https://gateway.example.com/v2'), false);
  assert.equal(secret.keepsAddress('https://gateway.example.com/v1', 'http://gateway.example.com/v1'), false);
  // Moving from HTTP to HTTPS on the same host adds no recipient.
  assert.equal(secret.keepsAddress('http://openclaw.example.com', 'https://openclaw.example.com'), true);
  assert.equal(secret.keepsAddress('http://openclaw.example.com:8080/a', 'https://openclaw.example.com:9090/a'), false);
  assert.equal(secret.keepsAddress('http://openclaw.example.com/a', 'https://openclaw.example.com/b'), false);
  assert.equal(secret.keepsAddress('not a url/', 'NOT A URL'), true);
  assert.equal(secret.keepsAddress('not a url', 'https://gateway.example.com'), false);
});

test('the key has to be entered again only when a stored key would follow a changed address', () => {
  const stored = { stored: true, hint: '7777' };
  const none = { stored: false, hint: null };
  const a = 'https://gateway.example.com/v1';
  const b = 'https://other.example.com/v1';
  assert.equal(secret.keyRequiredForAddress(a, b, stored, ''), true);
  assert.equal(secret.keyRequiredForAddress(a, b, stored, null), true);
  assert.equal(secret.keyRequiredForAddress(a, b, stored, '   '), true);
  assert.equal(secret.keyRequiredForAddress(a, b, stored, FAKE_KEY), false, 'a key typed for the new address is enough');
  assert.equal(secret.keyRequiredForAddress(a, b, none, ''), false, 'no stored key, nothing to bind');
  assert.equal(secret.keyRequiredForAddress(a, a + '/', stored, ''), false);
  assert.equal(secret.keyRequiredForAddress(a, '', stored, ''), false, 'an emptied field is not a destination');
});

test('after a key is removed the shared configuration shows the provider in use as having no key at once', () => {
  const stored = { stored: true, hint: '7777' };
  const none = { stored: false, hint: null };
  const config = { provider: 'openai', apiKeyStatus: stored, customOpenAIKeyStatus: stored };
  same(secret.withRefreshedKeyStatus(config, 'openai', none), { provider: 'openai', apiKeyStatus: none, customOpenAIKeyStatus: stored });
  // Another provider's status does not touch the active one.
  same(secret.withRefreshedKeyStatus(config, 'claude', none), config);
  // The custom provider has its own field, and also the active one when it is in use.
  same(secret.withRefreshedKeyStatus(config, 'custom-openai', none), { provider: 'openai', apiKeyStatus: stored, customOpenAIKeyStatus: none });
  same(
    secret.withRefreshedKeyStatus({ provider: 'custom-openai', apiKeyStatus: stored, customOpenAIKeyStatus: stored }, 'custom-openai', none),
    { provider: 'custom-openai', apiKeyStatus: none, customOpenAIKeyStatus: none },
  );
  assert.equal(config.apiKeyStatus, stored, 'the original is not modified');
});
