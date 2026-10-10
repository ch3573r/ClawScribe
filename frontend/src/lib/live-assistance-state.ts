import type { AssistantReply, AskRequest, ScopedDocument } from '@/types/knowledge';

export interface LiveSnapshot {
  session_id: string;
  finalized_through_seconds: number;
  segments: { sequence_id: number; text: string; start_seconds: number; end_seconds: number }[];
  transcription_incomplete: boolean;
  transcription_available: boolean;
}
export interface LiveReply extends AssistantReply {
  live_context?: { session_id: string; finalized_through_seconds: number; transcription_incomplete: boolean };
}
export interface LiveAskRequest extends Omit<AskRequest, 'owner' | 'search'> {
  owner: { kind: 'live'; id: string };
  search: { scope: { kind: 'live'; session_id: string }; query: string; document_ids: string[]; mode: 'keyword' };
  live_reference_scope?: { all_meetings: false; meeting_ids: string[]; tags: string[]; tag_mode: 'any'; untagged: false; from: null; to: null };
}
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

/** Compilable Task 8 RED stub; the global React provider will own this store. */
export function createLiveAssistanceStore(_dependencies: LiveAssistanceDependencies) {
  const state: LiveAssistanceState = {
    sessionId: null, snapshot: null, messages: [], pending: false,
    transcriptSharing: false, sharingReady: false, sharingBusy: false,
    documentSharing: false, documentSharingBusy: false, meetingIds: [], documentIds: [],
    documents: [], documentsLoading: false, provider: '', model: '',
    error: null, snapshotError: null, referenceError: null,
  };
  return {
    getSnapshot: () => state,
    subscribe: (_callback: () => void) => () => {},
    connect: () => () => {},
    setRecording: (_status: string) => {},
    configureProvider: (_provider: string, _model: string) => {},
    refresh: async () => {},
    setTranscriptSharing: async (_enabled: boolean) => {},
    setDocumentSharing: async (_enabled: boolean) => {},
    setReferences: async (_meetingIds: string[], _documentIds: string[]) => {},
    ask: async (_question: string) => {},
    cancel: () => {},
  };
}
