export type SearchMode = 'keyword' | 'hybrid';
export interface MeetingFilter {
  all_meetings: boolean; meeting_ids: string[]; tags: string[]; tag_mode: 'any' | 'all';
  untagged: boolean; from: string | null; to: string | null;
}
export type KnowledgeScope = { kind: 'meeting'; meeting_id: string } | { kind: 'library'; filter: MeetingFilter };
export type ConversationOwner = { kind: 'meeting' | 'library'; id: string };
export interface TextSpan { transcript_id: string; start_byte: number; end_byte: number }
export type EvidenceLocator = {kind:'transcript';meeting_id:string;transcript_ids:string[];spans:TextSpan[];start_seconds:number|null}
  | {kind:'document';document_id:string;page:number|null;paragraph:number}
  | {kind:'live';session_id:string;sequence_ids:number[]};
export interface EvidenceRef { historical?: boolean; source_id:string;source_revision:number;chunk_id:string;fingerprint:string;locator:EvidenceLocator }
export interface EvidenceDisplay {title:string;date:string;speaker:string|null;metadata_truncated:boolean;preceding_question_tag?:number|null}
export interface Passage extends Omit<EvidenceDisplay,'preceding_question_tag'> {evidence:EvidenceRef;meeting_id:string;text:string;rank:number}
export interface IndexStatus {keyword_ready:boolean;semantic_enabled:boolean;semantic_ready:number;pending:number;failed:number;reason:string|null}
export interface ModelDownloadStatus {
  stage:'idle'|'checking'|'downloading'|'verifying'|'cancelling'|'ready'|'cancelled'|'error';
  downloaded_bytes:number;total_bytes:number;current_file:string|null;error:string|null;
}
export interface ModelStatus {enabled:boolean;ready:boolean;installed:boolean;model:string;download:ModelDownloadStatus}
export interface SearchRequest {scope:KnowledgeScope;query:string;document_ids:string[];mode:SearchMode}
export interface SearchResponse {passages:Passage[];mode:SearchMode;index_status:IndexStatus}
export interface AskRequest {request_id:string;owner:ConversationOwner;search:SearchRequest}
export interface CitationContextLink {kind:'preceding_question';cited_tag:number;context_tag:number}
export interface AssistantReply {
  request_id:string;message_id:string;content:string;evidence:EvidenceRef[];evidence_metadata:EvidenceDisplay[];
  cited_tags:number[];context_links?:CitationContextLink[];retrieval_mode:SearchMode;provider:string;model:string;
}
export interface HistoryMessage {id:string;request_id:string|null;role:string;content:string;created_at:string;status:string;legacy:boolean;reply:AssistantReply|null}
export interface EvidenceNavigation {meeting_id:string;transcript_id:string;transcript_index:number;start_seconds:number|null}
export interface ResolvedEvidence {status:'current'|'stale'|'missing'|'invalid';passage:Passage|null;navigation:EvidenceNavigation|null}
