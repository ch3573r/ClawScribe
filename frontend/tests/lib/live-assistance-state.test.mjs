import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { deferred, flush } from './hook-harness.mjs';

const { createLiveAssistanceStore } = loadTsModule('src/lib/live-assistance-state.ts');
// Synthetic one-hour bilingual meeting, with a delayed final minute. The UI must
// retain only metadata and submit backend identity, never these transcript bodies.
const hour = Array.from({ length: 720 }, (_, i) => ({
  sequence_id: i + 1,
  text: i % 7 === 0 ? 'Sollen wir die Freigabe am Freitag prüfen? Ja.' : `Public fixture discussion ${i + 1}: review the rollout and unresolved checks.`,
  start_seconds: i * 5,
  end_seconds: (i + 1) * 5,
}));
const snapshot = id => ({ session_id: id, finalized_through_seconds: 3540,
  segments: hour.filter(segment => segment.end_seconds > 2940 && segment.end_seconds <= 3540),
  transcription_incomplete: true, transcription_available: true });
const document = (id, meeting) => ({ attachment: { id, display_name: 'Public reference', format: 'text',
  file_size: 40, sha256: 'fixture-hash', extraction_status: 'ready', indexing_status: 'ready' }, meeting_ids: [meeting] });
function fixture(overrides = {}, options = {}) {
  let backendId = 'session-one', sequence = 0;
  const requests = [], cancellations = [], registrations = [], timers = new Set(), handlers = new Map(), writes = [];
  const service = {
    liveSnapshot: async () => snapshot(backendId), liveSharing: async () => options.sharing ?? true,
    setLiveSharing: async enabled => { writes.push(['transcript', enabled]); },
    documentSharing: async () => false,
    setDocumentSharing: async (owner, enabled) => { writes.push(['documents', owner.id, enabled]); },
    scopeDocuments: async () => [document('doc-one', 'saved-one')],
    ask: async request => reply(request), cancel: async (id, session) => { cancellations.push([id, session]); },
    ...overrides,
  };
  const owner = createLiveAssistanceStore({
    service: { ...service, ask: request => { requests.push(JSON.parse(JSON.stringify(request))); return service.ask(request); } },
    listen: async (event, callback) => { registrations.push(event); handlers.set(event, callback); return () => handlers.delete(event); },
    interval: callback => { timers.add(callback); return () => timers.delete(callback); },
    uuid: () => `request-${++sequence}`,
  });
  owner.configureProvider('custom-openai', 'fixture-model');
  const disconnect = owner.connect();
  return { owner, requests, cancellations, registrations, timers, handlers, writes,
    async start(id = 'session-one', mode = 'live') { backendId = id; handlers.get('recording-started')?.({ session_id: id, recording_mode: mode }); owner.setRecording('recording'); await flush(); },
    disconnect,
  };
}
function reply(request, overrides = {}) {
  return { request_id: request.request_id, message_id: `answer-${request.request_id}`, content: 'Review remains open.',
    evidence: [], evidence_metadata: [], cited_tags: [], context_links: [], retrieval_mode: 'keyword',
    provider: 'custom-openai', model: 'fixture-model',
    live_context: { session_id: request.owner.id, finalized_through_seconds: 3540, transcription_incomplete: true }, ...overrides };
}
async function active(overrides, options) { const app = fixture(overrides, options); await flush(); await app.start(); return app; }

test('route_change_does_not_duplicate_listener', async () => {
  const answer = deferred(); const app = await active({ ask: () => answer.promise });
  try {
    const home = app.owner.subscribe(() => {}); const pending = app.owner.ask('Decision?');
    home(); const settings = app.owner.subscribe(() => {}); settings(); const homeAgain = app.owner.subscribe(() => {});
    assert.equal(app.registrations.filter(event => event === 'recording-started').length, 1);
    assert.equal(app.registrations.filter(event => event === 'recording-stopped').length, 1);
    assert.equal(app.registrations.includes('transcript-update'), false);
    assert.equal(app.requests.length, 1);
    answer.resolve(reply(app.requests[0])); await pending;
    assert.equal(app.owner.getSnapshot().messages.length, 1, 'route changes preserve the global request'); homeAgain();
  } finally { app.disconnect(); }
  assert.equal(app.handlers.size, 0); assert.equal(app.timers.size, 0);
});

test('stop_clears_live_messages', async () => {
  const answer = deferred(); let deferAnswer = false;
  const app = await active({ ask: request => deferAnswer ? answer.promise : Promise.resolve(reply(request)) });
  try {
    await app.owner.ask('Summarize so far'); assert.equal(app.owner.getSnapshot().messages.length, 1);
    await app.owner.setDocumentSharing(true); await app.owner.setReferences(['saved-one'], ['doc-one']);
    deferAnswer = true; const pending = app.owner.ask('What remains open?');
    app.owner.setRecording('stopping'); app.owner.setRecording('stopping');
    const state = app.owner.getSnapshot();
    assert.equal(state.sessionId, null); assert.equal(state.messages.length, 0); assert.equal(state.snapshot, null);
    assert.equal(state.pending, false); assert.equal(state.documentSharing, false); assert.equal(state.documentIds.length, 0);
    assert.equal(app.cancellations.length, 1, 'repeated Stop only signals once, before recording-stopped');
    answer.resolve(reply(app.requests.at(-1))); await pending; assert.equal(app.owner.getSnapshot().messages.length, 0);
  } finally { app.disconnect(); }
});

test('second_session_cannot_receive_first_reply', async () => {
  const first = deferred(); const app = await active({ ask: request => request.owner.id === 'session-one' ? first.promise : Promise.resolve(reply(request)) });
  try {
    const old = app.owner.ask('Old question'); assert.equal(app.requests.length, 1); app.owner.setRecording('stopping'); await app.start('session-two');
    await app.owner.ask('Current question'); first.resolve(reply(app.requests[0])); await old;
    assert.equal(app.owner.getSnapshot().sessionId, 'session-two'); assert.equal(app.owner.getSnapshot().messages.length, 1);
    assert.equal(app.owner.getSnapshot().messages[0].question, 'Current question');
    app.handlers.get('recording-stopped')?.({ session_id: 'session-one' });
    assert.equal(app.owner.getSnapshot().sessionId, 'session-two', 'late Stop event cannot clear the replacement');
  } finally { app.disconnect(); }
});

test('repeated_action_sends_once', async () => {
  const answer = deferred(); const app = await active({ ask: () => answer.promise });
  try {
    const first = app.owner.ask('Summarize so far'); const repeated = app.owner.ask('Summarize so far'); const different = app.owner.ask('List open questions');
    assert.equal(app.requests.length, 1); assert.equal(app.owner.getSnapshot().pending, true);
    answer.resolve(reply(app.requests[0])); await Promise.all([first, repeated, different]);
    assert.equal(app.owner.getSnapshot().messages.length, 1); assert.equal(app.owner.getSnapshot().pending, false);
  } finally { app.disconnect(); }
});

test('backlog_timestamp_stays_visible', async () => {
  const app = await active({ ask: async () => { throw 'provider failure'; } });
  try {
    const state = app.owner.getSnapshot(); assert.ok(state.snapshot, 'finalized backend metadata is available'); assert.equal(state.snapshot.finalized_through_seconds, 3540);
    assert.equal(state.snapshot.transcription_incomplete, true); assert.equal('segments' in state.snapshot, false);
    await app.owner.ask('What remains open?'); assert.match(app.owner.getSnapshot().error, /assistance|provider/i);
    assert.equal(app.owner.getSnapshot().snapshot.finalized_through_seconds, 3540);
    assert.equal(app.owner.getSnapshot().snapshot.transcription_incomplete, true);
  } finally { app.disconnect(); }
});

test('remembered transcript consent and session document consent are independent', async () => {
  const app = await active({}, { sharing: false });
  try {
    assert.equal(app.owner.getSnapshot().transcriptSharing, false);
    await app.owner.setDocumentSharing(true); await app.owner.ask('Question'); assert.equal(app.requests.length, 0);
    await app.owner.setTranscriptSharing(true); await app.owner.ask('Question'); assert.equal(app.requests.length, 1);
    assert.deepEqual(app.writes, [['documents', 'session-one', true], ['transcript', true]]);
    app.owner.setRecording('stopping'); await app.start('session-two');
    assert.equal(app.owner.getSnapshot().transcriptSharing, true); assert.equal(app.owner.getSnapshot().documentSharing, false);
  } finally { app.disconnect(); }
});

test('reference selection is explicit and never submits UI transcript bodies', async () => {
  const app = await active();
  try {
    await app.owner.setReferences(['saved-one'], ['doc-one', 'unselected-document']);
    await app.owner.ask('Decision?');
    assert.equal(app.requests.length, 1);
    assert.deepEqual(app.requests[0].search.document_ids, [], 'document permission defaults off');
    await app.owner.setDocumentSharing(true); await app.owner.setReferences(['saved-one'], ['doc-one', 'unselected-document']); await app.owner.ask('Decision?');
    const request = app.requests[1]; assert.deepEqual(request.owner, { kind: 'live', id: 'session-one' });
    assert.deepEqual(request.search, { scope: { kind: 'live', session_id: 'session-one' }, query: 'Decision?', document_ids: ['doc-one'], mode: 'keyword' });
    assert.deepEqual(request.live_reference_scope, { all_meetings: false, meeting_ids: ['saved-one'], tags: [], tag_mode: 'any', untagged: false, from: null, to: null });
    assert.equal(JSON.stringify(request).includes('Public fixture discussion'), false);
    await app.owner.setReferences([], ['doc-one']); await app.owner.ask('Current recording only');
    assert.equal('live_reference_scope' in app.requests[2], false); assert.deepEqual(app.requests[2].search.document_ids, []);
  } finally { app.disconnect(); }
});

test('consent revocation rejects late completion and failed persistence stays disabled', async () => {
  const answer = deferred(), permission = deferred();
  const app = await active({ ask: () => answer.promise, setLiveSharing: () => permission.promise });
  try {
    const ask = app.owner.ask('Decision?'); const revoke = app.owner.setTranscriptSharing(false);
    assert.equal(app.owner.getSnapshot().pending, false); assert.equal(app.owner.getSnapshot().transcriptSharing, false);
    assert.equal(app.cancellations.length, 1); answer.resolve(reply(app.requests[0])); await ask;
    permission.reject('write failed'); await revoke; assert.equal(app.owner.getSnapshot().messages.length, 0);
    assert.equal(app.owner.getSnapshot().transcriptSharing, false); assert.match(app.owner.getSnapshot().error, /sharing/i);
  } finally { app.disconnect(); }
});

test('failed enable and stale consent initialization cannot enable sharing', async () => {
  const initial = deferred(); const app = fixture({ liveSharing: () => initial.promise, setLiveSharing: async () => { throw 'write failed'; } });
  try {
    await app.start(); await app.owner.setTranscriptSharing(true); initial.resolve(true); await flush();
    assert.equal(app.owner.getSnapshot().transcriptSharing, false); assert.match(app.owner.getSnapshot().error, /sharing/i);
    await app.owner.ask('Decision?'); assert.equal(app.requests.length, 0);
  } finally { app.disconnect(); }
});

test('provider and cancellation errors preserve recording metadata and allow manual retry', async () => {
  let fail = true; const answer = deferred();
  const app = await active({ ask: request => fail ? Promise.reject('provider failure') : answer.promise,
    cancel: async () => { throw 'cancel failed'; } });
  try {
    await app.owner.ask('Decision?'); assert.equal(app.owner.getSnapshot().pending, false); assert.equal(app.owner.getSnapshot().sessionId, 'session-one');
    assert.match(app.owner.getSnapshot().error, /assistance|provider/i); fail = false;
    const next = app.owner.ask('Decision?'); app.owner.cancel(); await flush();
    assert.equal(app.owner.getSnapshot().pending, false); assert.match(app.owner.getSnapshot().error, /cancel/i);
    assert.equal(app.owner.getSnapshot().snapshot.finalized_through_seconds, 3540);
    answer.resolve(reply(app.requests[1])); await next; assert.equal(app.owner.getSnapshot().messages.length, 0);
  } finally { app.disconnect(); }
});

test('snapshot, references and permission operations cannot restore a stopped session', async () => {
  const read = deferred(), docs = deferred(), permission = deferred(); let slow = false;
  const app = await active({ liveSnapshot: () => slow ? read.promise : Promise.resolve(snapshot('session-one')),
    scopeDocuments: () => docs.promise, setDocumentSharing: () => permission.promise });
  try {
    assert.equal(app.owner.getSnapshot().sessionId, 'session-one');
    slow = true; const refresh = app.owner.refresh(); const references = app.owner.setReferences(['saved-one'], ['doc-one']); const sharing = app.owner.setDocumentSharing(true);
    app.owner.setRecording('stopping'); read.resolve(snapshot('session-one')); docs.resolve([document('doc-one', 'saved-one')]); permission.resolve();
    await Promise.all([refresh, references, sharing]); assert.equal(app.owner.getSnapshot().sessionId, null);
    assert.equal(app.owner.getSnapshot().snapshot, null); assert.equal(app.owner.getSnapshot().documentIds.length, 0); assert.equal(app.owner.getSnapshot().documentSharing, false);
  } finally { app.disconnect(); }
});

test('mismatched request and session replies are rejected without an assistant message', async () => {
  for (const mismatch of ['request', 'session']) {
    const app = await active({ ask: request => Promise.resolve(reply(request, mismatch === 'request' ? { request_id: 'foreign-request' } : { live_context: { session_id: 'foreign-session', finalized_through_seconds: 3540, transcription_incomplete: true } })) });
    try { await app.owner.ask('Decision?'); assert.equal(app.owner.getSnapshot().messages.length, 0); assert.match(app.owner.getSnapshot().error, /session|reply/i); }
    finally { app.disconnect(); }
  }
});

test('audio-only and Built-in AI never dispatch a live provider request', async () => {
  const app = await active({ liveSnapshot: async () => ({ ...snapshot('session-one'), segments: [], transcription_available: false }) });
  try {
    await app.owner.ask('Decision?'); assert.equal(app.requests.length, 0); assert.match(app.owner.getSnapshot().error, /transcript|audio/i);
    app.owner.configureProvider('builtin-ai', 'local-model'); await app.owner.ask('Decision?'); assert.equal(app.requests.length, 0);
    assert.match(app.owner.getSnapshot().error, /Built-in AI|recording/i);
  } finally { app.disconnect(); }
});

test('provider change cancels an in-flight answer and bounded ephemeral history does not grow indefinitely', async () => {
  const answer = deferred(); let slow = true;
  const app = await active({ ask: request => slow ? answer.promise : Promise.resolve(reply(request)) });
  try {
    const first = app.owner.ask('Decision?'); app.owner.configureProvider('ollama', 'other-model');
    assert.equal(app.cancellations.length, 1); answer.resolve(reply(app.requests[0])); await first;
    assert.equal(app.owner.getSnapshot().messages.length, 0); slow = false;
    for (let i = 0; i < 80; i++) await app.owner.ask(`Question ${i}?`);
    assert.ok(app.owner.getSnapshot().messages.length > 0); assert.ok(app.owner.getSnapshot().messages.length <= 32);
  } finally { app.disconnect(); }
});

test('disconnect releases registrations that finish after cleanup and rejects pending replies', async () => {
  const subscriptions = [], answer = deferred(); let released = 0;
  const app = fixture({ ask: () => answer.promise }); await flush(); await app.start();
  const pending = app.owner.ask('Decision?'); assert.equal(app.requests.length, 1); app.disconnect(); answer.resolve(reply(app.requests[0])); await pending;
  assert.equal(app.owner.getSnapshot().messages.length, 0); assert.equal(app.handlers.size, 0); assert.equal(app.timers.size, 0);
  const owner = createLiveAssistanceStore({ service: { liveSharing: async () => false, liveSnapshot: async () => { throw 'not recording'; } },
    listen: () => { const late = deferred(); subscriptions.push(late); return late.promise; }, interval: () => () => {}, uuid: () => 'unused' });
  const cleanup = owner.connect(); cleanup(); subscriptions.forEach(subscription => subscription.resolve(() => released++)); await flush();
  assert.equal(subscriptions.length, 2); assert.equal(released, 2);
});
