import { invoke } from '@tauri-apps/api/core';
import type { AskRequest, ConversationOwner, DocumentAttachment, DocumentBlock, EvidenceRef, HistoryMessage, IndexStatus, KnowledgeScope, LibraryConversation, LiveAskRequest, LiveReply, LiveSnapshot, MeetingFilter, ModelStatus, ResolvedEvidence, ScopedDocument, SearchRequest, SearchResponse } from '@/types/knowledge';
import type { SavedReference } from '@/lib/archive-search';
export interface NotePreview {
  token: string;
  question: string;
  excerpts: { source: 'note' | 'bookmark'; text: string }[];
  destination: { provider: string; model: string; endpoint: string | null; organization: string | null; project: string | null };
}

/** The single native boundary for meeting memory. No provider credentials enter the UI request. */
export const knowledgeService = {
  search: (request:SearchRequest) => invoke<SearchResponse>('knowledge_search',{request}),
  ask: (request:AskRequest | LiveAskRequest) => invoke<LiveReply>('knowledge_ask',{request}),
  previewNotes: (request: AskRequest, references: SavedReference[]) => invoke<NotePreview>('knowledge_preview_notes', { request, references }),
  askWithNotes: (request: AskRequest, references: SavedReference[], token: string) => invoke<LiveReply>('knowledge_ask_with_notes', { request, references, token }),
  discardNoteReview: (token: string) => invoke<void>('knowledge_discard_note_review', { token }),
  cancel: (requestId:string,sessionId?:string) => invoke<void>('knowledge_cancel_request',{requestId,...(sessionId ? {sessionId} : {})}),
  liveSnapshot: () => invoke<LiveSnapshot>('knowledge_live_snapshot'),
  liveSharing: () => invoke<boolean>('knowledge_live_sharing'),
  setLiveSharing: (enabled:boolean) => invoke<void>('knowledge_set_live_sharing',{enabled}),
  history: (owner:ConversationOwner) => invoke<HistoryMessage[]>('knowledge_history',{owner}),
  clear: (owner:ConversationOwner) => invoke<number>('knowledge_clear_history',{owner}),
  createConversation: () => invoke<ConversationOwner>('knowledge_create_library_conversation'),
  conversations: () => invoke<LibraryConversation[]>('knowledge_list_library_conversations'),
  renameConversation: (owner:ConversationOwner,title:string) => invoke<void>('knowledge_rename_library_conversation',{owner,title}),
  deleteConversation: (owner:ConversationOwner) => invoke<void>('knowledge_delete_library_conversation',{owner}),
  conversationScope: (owner:ConversationOwner) => invoke<MeetingFilter | null>('knowledge_library_conversation_scope',{owner}),
  resolve: (reference:EvidenceRef) => invoke<ResolvedEvidence>('knowledge_resolve_evidence',{reference}),
  indexStatus: () => invoke<IndexStatus>('knowledge_index_status'),
  modelStatus: () => invoke<ModelStatus>('knowledge_model_status'),
  enable: (enabled:boolean) => invoke<void>('knowledge_model_enable',{enabled}),
  download: () => invoke<void>('knowledge_model_download'),
  cancelDownload: () => invoke<void>('knowledge_model_cancel_download'),
  reindex: (scope:KnowledgeScope) => invoke<void>('knowledge_reindex',{scope}),
  pause: (scope:KnowledgeScope) => invoke<void>('knowledge_cancel_index',{scope}),
  importDocument: (meetingId:string,path:string,requestId:string) => invoke<DocumentAttachment>('knowledge_import_document',{meetingId,path,requestId}),
  cancelDocumentImport: (requestId:string) => invoke<void>('knowledge_cancel_document_import',{requestId}),
  listDocuments: (meetingId:string) => invoke<DocumentAttachment[]>('knowledge_list_documents',{meetingId}),
  scopeDocuments: (scope:KnowledgeScope) => invoke<ScopedDocument[]>('knowledge_list_scope_documents',{scope}),
  documentBlocks: (meetingId:string,documentId:string) => invoke<DocumentBlock[]>('knowledge_get_document_blocks',{meetingId,documentId}),
  detachDocument: (meetingId:string,documentId:string) => invoke<void>('knowledge_remove_attachment',{meetingId,documentId}),
  deleteDocument: (meetingId:string,documentId:string) => invoke<void>('knowledge_delete_document',{meetingId,documentId}),
  retryDocument: (meetingId:string,documentId:string) => invoke<void>('knowledge_retry_document_index',{meetingId,documentId}),
  documentSharing: (owner:ConversationOwner | {kind:'live';id:string}) => invoke<boolean>('knowledge_document_sharing',{owner}),
  setDocumentSharing: (owner:ConversationOwner | {kind:'live';id:string},enabled:boolean) => invoke<void>('knowledge_set_document_sharing',{owner,enabled}),
};
