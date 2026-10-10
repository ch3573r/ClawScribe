import { invoke } from '@tauri-apps/api/core';
import type { AskRequest, AssistantReply, ConversationOwner, DocumentAttachment, DocumentBlock, EvidenceRef, HistoryMessage, IndexStatus, KnowledgeScope, ModelStatus, ResolvedEvidence, ScopedDocument, SearchRequest, SearchResponse } from '@/types/knowledge';

/** The single native boundary for meeting memory. No provider credentials enter the UI request. */
export const knowledgeService = {
  search: (request:SearchRequest) => invoke<SearchResponse>('knowledge_search',{request}),
  ask: (request:AskRequest) => invoke<AssistantReply>('knowledge_ask',{request}),
  cancel: (requestId:string) => invoke<void>('knowledge_cancel_request',{requestId}),
  history: (owner:ConversationOwner) => invoke<HistoryMessage[]>('knowledge_history',{owner}),
  clear: (owner:ConversationOwner) => invoke<number>('knowledge_clear_history',{owner}),
  createConversation: () => invoke<ConversationOwner>('knowledge_create_library_conversation'),
  conversations: () => invoke<ConversationOwner[]>('knowledge_list_library_conversations'),
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
  documentSharing: (owner:ConversationOwner) => invoke<boolean>('knowledge_document_sharing',{owner}),
  setDocumentSharing: (owner:ConversationOwner,enabled:boolean) => invoke<void>('knowledge_set_document_sharing',{owner,enabled}),
};
