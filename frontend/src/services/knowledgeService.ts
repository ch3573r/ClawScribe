import { invoke } from '@tauri-apps/api/core';
import type { AskRequest, AssistantReply, ConversationOwner, EvidenceRef, HistoryMessage, IndexStatus, KnowledgeScope, ModelStatus, ResolvedEvidence, SearchRequest, SearchResponse } from '@/types/knowledge';

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
};
