import type { LiveAskRequest, LiveReply, LiveSnapshot, ScopedDocument } from '@/types/knowledge';
export interface LiveAssistanceState {
  sessionId: string | null;
  snapshot: Omit<LiveSnapshot, 'segments'> | null;
  messages: { question: string; reply: LiveReply }[];
  pending: boolean;
  transcriptSharing: boolean;
  sharingReady: boolean;
  sharingBusy: boolean;
  documentSharing: boolean;
  documentSharingBusy: boolean;
  meetingIds: string[];
  documentIds: string[];
  documents: ScopedDocument[];
  documentsLoading: boolean;
  provider: string;
  model: string;
  error: string | null;
  snapshotError: string | null;
  referenceError: string | null;
}
export interface LiveAssistanceDependencies {
  service: {
    liveSnapshot(): Promise<LiveSnapshot>;
    liveSharing(): Promise<boolean>;
    setLiveSharing(enabled: boolean): Promise<void>;
    documentSharing(owner: { kind: 'live'; id: string }): Promise<boolean>;
    setDocumentSharing(owner: { kind: 'live'; id: string }, enabled: boolean): Promise<void>;
    scopeDocuments(scope: { kind: 'library'; filter: NonNullable<LiveAskRequest['live_reference_scope']> }): Promise<ScopedDocument[]>;
    ask(request: LiveAskRequest): Promise<LiveReply>;
    cancel(id: string, sessionId: string): Promise<void>;
  };
  listen(event: string, callback: (payload: { session_id?: string; recording_mode?: string }) => void): Promise<() => void>;
  interval(callback: () => void, milliseconds: number): () => void;
  uuid(): string;
}

/** One app-owned store; route subscribers never own native events or requests. */
export function createLiveAssistanceStore(dependencies: LiveAssistanceDependencies) {
  const { service } = dependencies;
  let state: LiveAssistanceState = {
    sessionId: null, snapshot: null, messages: [], pending: false,
    transcriptSharing: false, sharingReady: false, sharingBusy: false,
    documentSharing: false, documentSharingBusy: false, meetingIds: [], documentIds: [],
    documents: [], documentsLoading: false, provider: '', model: '',
    error: null, snapshotError: null, referenceError: null,
  };
  const subscribers = new Set<() => void>();
  const update = (patch: Partial<LiveAssistanceState>) => {
    state = { ...state, ...patch };
    subscribers.forEach(callback => callback());
  };
  let generation = 0;
  let permissionRevision = 0;
  let documentRevision = 0;
  let referenceRevision = 0;
  let snapshotRevision = 0;
  let recording = 'idle';
  let connected = false;
  let connectionRevision = 0;
  let reading: Promise<void> | null = null;
  let pending: { id: string; sessionId: string; promise: Promise<void> } | null = null;
  const current = (version: number, sessionId: string | null) =>
    generation === version && state.sessionId === sessionId;
  const cancel = (showFailure = true) => {
    const request = pending;
    pending = null;
    if (state.pending) update({ pending: false });
    if (!request) return;
    const version = generation;
    void service.cancel(request.id, request.sessionId).catch(() => {
      if (showFailure && current(version, request.sessionId) && !pending)
        update({ error: 'Could not confirm assistance cancellation. Late replies are hidden; retry or check the provider.' });
    });
  };
  const clearSession = () => {
    generation++;
    snapshotRevision++;
    documentRevision++;
    referenceRevision++;
    reading = null;
    cancel(false);
    update({ sessionId: null, snapshot: null, messages: [], documentSharing: false,
      documentSharingBusy: false, documents: [], documentsLoading: false,
      meetingIds: [], documentIds: [], error: null, snapshotError: null, referenceError: null });
  };
  const startSession = (id: string) => {
    if (state.sessionId === id) return;
    clearSession();
    update({ sessionId: id });
    const version = generation;
    const revision = documentRevision;
    void service.documentSharing({ kind: 'live', id }).then(enabled => {
      if (current(version, id) && revision === documentRevision) update({ documentSharing: enabled });
    }).catch(() => {
      if (current(version, id) && revision === documentRevision)
        update({ referenceError: 'Could not read reference sharing. It remains off; retry the switch.' });
    });
  };
  const filter = (meetingIds: string[]): NonNullable<LiveAskRequest['live_reference_scope']> => ({
    all_meetings: false, meeting_ids: [...meetingIds], tags: [], tag_mode: 'any', untagged: false, from: null, to: null,
  });
  const refresh = (): Promise<void> => {
    if (reading) return reading;
    if (['stopping', 'processing', 'saving', 'completed', 'error', 'starting'].includes(recording)) return Promise.resolve();
    const version = generation;
    const revision = ++snapshotRevision;
    const work = service.liveSnapshot().then(snapshot => {
      if (generation !== version || snapshotRevision !== revision) return;
      if (state.sessionId && snapshot.session_id !== state.sessionId) return;
      if (!state.sessionId) startSession(snapshot.session_id);
      // Text is native evidence only. Retain bounded metadata, never a second UI transcript.
      const { segments: _segments, ...metadata } = snapshot;
      update({ snapshot: metadata, snapshotError: null });
    }).catch(() => {
      if (generation === version && snapshotRevision === revision && state.sessionId)
        update({ snapshotError: 'Could not read finalized transcript status. Retry before asking.' });
    }).finally(() => { if (reading === work) reading = null; });
    reading = work;
    return work;
  };
  const api = {
    getSnapshot: () => state,
    subscribe(callback: () => void) { subscribers.add(callback); return () => { subscribers.delete(callback); }; },
    connect() {
      if (connected) return () => {};
      connected = true;
      const connection = ++connectionRevision;
      let disposed = false;
      const unlisteners: (() => void)[] = [];
      const register = (event: string, callback: (payload: { session_id?: string; recording_mode?: string }) => void) => {
        void dependencies.listen(event, payload => { if (!disposed) callback(payload); }).then(unlisten => {
          if (disposed) unlisten(); else unlisteners.push(unlisten);
        }).catch(() => {
          if (!disposed) update({ snapshotError: 'Could not watch recording changes. Reopen ClawScribe before using live assistance.' });
        });
      };
      register('recording-started', payload => {
        if (!payload.session_id) return;
        recording = 'recording';
        startSession(payload.session_id);
        void refresh();
      });
      register('recording-stopped', payload => {
        if (payload.session_id && payload.session_id !== state.sessionId) return;
        recording = 'stopping';
        clearSession();
      });
      const revision = permissionRevision;
      void service.liveSharing().then(enabled => {
        if (!disposed && revision === permissionRevision) update({ transcriptSharing: enabled, sharingReady: true });
      }).catch(() => {
        if (!disposed && revision === permissionRevision)
          update({ sharingReady: true, transcriptSharing: false, error: 'Could not read live sharing. It remains off; retry the switch.' });
      });
      // Covers an active backend recording after a WebView reload. Events are
      // registered first and every read is guarded against a newer lifecycle.
      void refresh();
      const stopInterval = dependencies.interval(() => { if (state.sessionId || recording === 'recording') void refresh(); }, 5000);
      return () => {
        if (disposed || connection !== connectionRevision) return;
        disposed = true;
        connected = false;
        stopInterval();
        unlisteners.forEach(unlisten => unlisten());
        recording = 'idle';
        clearSession();
      };
    },
    setRecording(status: string) {
      if (status === recording) return;
      recording = status;
      if (status === 'recording') void refresh();
      else if (status !== 'starting') clearSession();
    },
    configureProvider(provider: string, model: string) {
      if (state.provider === provider && state.model === model) return;
      cancel(false);
      update({ provider, model, error: null });
    },
    refresh,
    async setTranscriptSharing(enabled: boolean) {
      if (state.sharingBusy) return;
      const revision = ++permissionRevision;
      cancel(false);
      update({ sharingBusy: true, sharingReady: true, transcriptSharing: false, error: null });
      try {
        await service.setLiveSharing(enabled);
        if (permissionRevision === revision) update({ transcriptSharing: enabled });
      } catch {
        if (permissionRevision === revision)
          update({ transcriptSharing: false, error: 'Could not save live sharing. It remains off; retry.' });
      } finally { if (permissionRevision === revision) update({ sharingBusy: false }); }
    },
    async setDocumentSharing(enabled: boolean) {
      const id = state.sessionId;
      if (!id || state.documentSharingBusy) return;
      const version = generation;
      const revision = ++documentRevision;
      cancel(false);
      update({ documentSharing: false, documentSharingBusy: true, referenceError: null });
      try {
        await service.setDocumentSharing({ kind: 'live', id }, enabled);
        if (current(version, id) && documentRevision === revision) update({ documentSharing: enabled });
      } catch {
        if (current(version, id) && documentRevision === revision)
          update({ documentSharing: false, referenceError: 'Could not update reference sharing. It remains off; retry.' });
      } finally { if (current(version, id) && documentRevision === revision) update({ documentSharingBusy: false }); }
    },
    async setReferences(meetingIds: string[], documentIds: string[]) {
      const id = state.sessionId;
      if (!id) return;
      cancel(false);
      const ids = [...new Set(meetingIds)].filter(value => value.trim());
      const version = generation;
      const revision = ++referenceRevision;
      const sameScope = JSON.stringify(state.meetingIds) === JSON.stringify(ids);
      const eligible = (documents: ScopedDocument[]) => documentIds.filter(docId =>
        documents.some(row => row.attachment.id === docId && row.meeting_ids.some(meetingId => ids.includes(meetingId))));
      update({ meetingIds: ids, documentIds: sameScope ? eligible(state.documents) : [],
        documents: sameScope ? state.documents : [], documentsLoading: !!ids.length, referenceError: null });
      if (!ids.length) { update({ documentIds: [], documentsLoading: false }); return; }
      try {
        const documents = await service.scopeDocuments({ kind: 'library', filter: filter(ids) });
        if (current(version, id) && referenceRevision === revision)
          update({ documents, documentIds: eligible(documents), referenceError: null });
      } catch {
        if (current(version, id) && referenceRevision === revision)
          update({ documentIds: [], referenceError: 'Could not load selected references. Retry before asking.' });
      } finally { if (current(version, id) && referenceRevision === revision) update({ documentsLoading: false }); }
    },
    ask(question: string): Promise<void> {
      if (pending) return pending.promise;
      const text = question.trim();
      if (!text) return Promise.resolve();
      const snapshot = state.snapshot;
      const sessionId = state.sessionId;
      const unavailable = state.provider === 'builtin-ai' ? 'Built-in AI is available after recording. Open the saved meeting to ask.'
        : !sessionId || !snapshot?.transcription_available ? 'No live transcript is available. Ask after transcription in the saved meeting.'
        : !state.provider || !state.model ? 'Choose a summary provider and model in Settings.'
        : !state.sharingReady || !state.transcriptSharing || state.sharingBusy ? 'Enable live transcript sharing before asking.'
        : state.documentSharingBusy || state.documentsLoading || state.referenceError && state.meetingIds.length ? 'Wait for references or retry their error before asking.'
        : state.snapshotError ? 'Retry finalized transcript status before asking.'
        : snapshot.finalized_through_seconds <= 0 ? 'Waiting for finalized transcript. Try again when speech has been transcribed.'
        : utf8Bytes(text) > 1024 ? 'Keep your question within 1,024 UTF-8 bytes.' : null;
      if (unavailable || !sessionId) { update({ error: unavailable }); return Promise.resolve(); }
      const version = generation;
      const id = dependencies.uuid();
      const request: LiveAskRequest = { request_id: id, owner: { kind: 'live', id: sessionId }, search: {
        scope: { kind: 'live', session_id: sessionId }, query: text,
        document_ids: state.documentSharing ? [...state.documentIds] : [], mode: 'keyword',
      }, ...(state.meetingIds.length ? { live_reference_scope: filter(state.meetingIds) } : {}) };
      const entry = { id, sessionId, promise: Promise.resolve() };
      pending = entry;
      update({ pending: true, error: null });
      const valid = () => current(version, sessionId) && pending === entry;
      entry.promise = service.ask(request).then(reply => {
        if (!valid()) return;
        if (reply.request_id !== id || reply.live_context?.session_id !== sessionId) {
          update({ error: 'This assistance reply belongs to a different request or session. Ask again.' }); return;
        }
        const messages = [...state.messages, { question: text, reply }].slice(-32);
        while (messages.length && utf8Bytes(JSON.stringify(messages)) > 65536) messages.shift();
        update({ messages });
      }).catch(() => {
        if (valid()) update({ error: 'Live assistance could not answer. Check the provider connection or resources and retry. Recording status is shown separately.' });
      }).finally(() => { if (valid()) { pending = null; update({ pending: false }); } });
      return entry.promise;
    },
    cancel() { cancel(); },
  };
  return api;
}

function utf8Bytes(text: string) {
  let bytes = 0;
  for (const character of text) {
    const code = character.codePointAt(0)!;
    bytes += code <= 0x7f ? 1 : code <= 0x7ff ? 2 : code <= 0xffff ? 3 : 4;
  }
  return bytes;
}
