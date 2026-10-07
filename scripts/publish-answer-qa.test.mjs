import test from 'node:test';
import assert from 'node:assert/strict';
import { reportBatches } from './publish-answer-qa.mjs';
const sha = 'a'.repeat(40);
const record = value => `\n\x60\x60\x60json\n${JSON.stringify(value)}\n\x60\x60\x60\n`;
const header = {label:'PUBLIC SYNTHETIC EVALUATION — actual first attempts; independent factual review pending',build_sha:sha,helper_sha256:'b'.repeat(64),fixture:{},model_catalog:{}};
test('synthetic reporting preserves all Unicode bytes in bounded fields', () => {
 const source = record(header) + record({case:'01',actual_reply:'Grüße '.repeat(24000)});
 const parts = reportBatches(source,sha);
 assert.equal(parts.join(''),source);
 assert.ok(parts.length > 1 && parts.length <= 16);
 assert.ok(parts.every(part => Buffer.byteLength(part) <= 58000));
});
test('synthetic reporting rejects mismatched SHA, non-evaluation data and oversized output', () => {
 assert.throws(()=>reportBatches(record({...header,build_sha:'c'.repeat(40)}),sha));
 assert.throws(()=>reportBatches('ordinary log output',sha));
 assert.throws(()=>reportBatches(record(header)+'x'.repeat(750*1024),sha));
});
