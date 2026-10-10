"use client";
import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { createKnowledgeController, scopeReady } from "@/lib/knowledge-state";
import { knowledgeService } from "@/services/knowledgeService";
import type {
  ConversationOwner,
  EvidenceDisplay,
  EvidenceRef,
  HistoryMessage,
  KnowledgeScope,
  ResolvedEvidence,
  SearchMode,
  SearchResponse,
  ScopedDocument,
} from "@/types/knowledge";

const errorText = (error: unknown) =>
  typeof error === "string"
    ? error
    : error instanceof Error
      ? error.message
      : "Meeting memory is unavailable. Retry the operation.";
export function useKnowledgeSearch(
  scope: KnowledgeScope,
  initialOwner: ConversationOwner | null = null,
  answerConfiguration?: { provider?: string; model?: string },
) {
  const [controller] = useState(() =>
    createKnowledgeController((id, current) => {
      void knowledgeService.cancel(id).catch((e) => {
        if (current())
          setError(
            `Answer cancelled in this view; native cancellation reported: ${errorText(e)}`,
          );
      });
    }),
  );
  // Keep the selected library thread across meeting-only/library scope toggles.
  const [savedOwner, setOwner] = useState<ConversationOwner | null>(
    initialOwner?.kind === "library" ? initialOwner : null,
  );
  const owner = initialOwner ?? savedOwner;
  const [threads, setThreads] = useState<ConversationOwner[]>([]);
  const [response, setResponse] = useState<SearchResponse | null>(null);
  const [messages, setMessages] = useState<HistoryMessage[]>([]);
  const [loading, setLoading] = useState(false);
  const [sending, setSending] = useState(false);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [error, setError] = useState("");
  const [eventError, setEventError] = useState("");
  const [libraryRevision, setLibraryRevision] = useState(0);
  const [documentRevision, setDocumentRevision] = useState(0);
  const [documentRows, setDocumentRows] = useState<{scope: string; rows: ScopedDocument[]}>({scope:'',rows:[]});
  const [chosenDocuments, setChosenDocuments] = useState<string[]>([]);
  const [documentsLoading, setDocumentsLoading] = useState(false);
  const [documentsError, setDocumentsError] = useState('');
  const [sharing, setSharing] = useState<{owner:string;enabled:boolean}>({owner:'',enabled:false});
  const [sharingLoading, setSharingLoading] = useState(false);
  const [sharingSaving, setSharingSaving] = useState(false);
  const [sharingError, setSharingError] = useState('');
  const sharingPending = useRef(false);
  const sharingRead = useRef(0);
  const sharingAdoptionError = useRef<{ ownerId: string; error: string } | null>(null);
  const documentScope = JSON.stringify({scope,libraryRevision});
  const ownerIdentity = JSON.stringify(owner);
  const documents = documentRows.scope === documentScope ? documentRows.rows : [];
  const selectedDocumentIds = chosenDocuments.filter(id => documents.some(row => row.attachment.id === id));
  const sharingEnabled = sharing.owner === ownerIdentity && sharing.enabled;
  const [creating, setCreating] = useState(false);
  const createPending = useRef(false);
  const adoptionError = useRef<{ ownerId: string; error: string } | null>(null);
  const adoptedRetry = useRef<{ identity: string; question: string; id: string } | null>(null);
  const [preview, setPreview] = useState<{
    reference: EvidenceRef;
    metadata: EvidenceDisplay;
    resolved: ResolvedEvidence | null;
    contextReference?: EvidenceRef;
  } | null>(null);
  const [previewBusy, setPreviewBusy] = useState(false);
  const identity = JSON.stringify({
    scope,
    owner,
    libraryRevision,
    answerConfiguration,
    selectedDocumentIds,
  });
  // Only automatic owner adoption may carry a failed first turn across identities.
  // Scope, provider, library revision, and ordinary thread changes discard it.
  if (adoptedRetry.current && adoptedRetry.current.identity !== identity) adoptedRetry.current = null;
  controller.configure({ scope, owner, libraryRevision, answerConfiguration, selectedDocumentIds });
  const [visibleFor, setVisibleFor] = useState(identity);
  const reset = () => {
    controller.invalidate();
    adoptedRetry.current = null;
    setResponse(null);
    setLoading(false);
    setSending(false);
    setCreating(false);
    setHistoryLoading(false);
    setPreview(null);
    setPreviewBusy(false);
    setError("");
  };
  useEffect(() => {
    setVisibleFor(identity);
    setResponse(null);
    setPreview(null);
    setPreviewBusy(false);
    setMessages([]);
    setLoading(false);
    setSending(false);
    setCreating(false);
    setError(adoptionError.current && adoptionError.current.ownerId === owner?.id ? adoptionError.current.error : "");
    adoptionError.current = null;
    if (!owner) {
      setHistoryLoading(false);
      return () => controller.invalidate();
    }
    setHistoryLoading(true);
    const current = controller.ticket("history-operation");
    void controller
      .run(
        "history",
        () => knowledgeService.history(owner),
        (value) => {
          setMessages(value);
          setHistoryLoading(false);
        },
      )
      .catch((e) => {
        if (current()) {
          setError(errorText(e));
          setHistoryLoading(false);
        }
      });
    return () => controller.invalidate();
    // JSON identity keeps equal scope objects from restarting history on every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [identity, controller]);
  useEffect(() => {
    if (scope.kind !== "library" || owner?.kind === "meeting") return;
    const current = controller.ticket("threads-operation");
    void controller
      .run("threads", knowledgeService.conversations, setThreads)
      .catch((e) => {
        if (current()) setError(errorText(e));
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [identity, controller]);
  useEffect(() => {
    let active = true;
    const subscription = listen("library-changed", () => {
      if (active) {
        controller.invalidate();
        setLibraryRevision((value) => value + 1);
      }
    });
    void subscription.catch(() => {
      if (active)
        setEventError(
          "Automatic source refresh is unavailable. Search again or reload history after editing meetings.",
        );
    });
    return () => {
      active = false;
      void subscription.then((unlisten) => unlisten()).catch(() => {});
    };
  }, [controller]);
  // Scope reads have their own lifetime: changing a selected document must not
  // cancel a still-needed list or consent read and leave its loading state stuck.
  useEffect(() => {
    let active = true;
    setDocumentsError('');
    if (!scopeReady(scope)) {
      setDocumentRows({scope:documentScope,rows:[]});
      setChosenDocuments([]);
      setDocumentsLoading(false);
      return () => { active = false; };
    }
    setDocumentsLoading(true);
    void knowledgeService.scopeDocuments(scope).then(rows => {
      if (!active) return;
      setDocumentRows({scope:documentScope,rows});
      setChosenDocuments(ids => ids.filter(id => rows.some(row => row.attachment.id === id)));
    }).catch(() => {
      if (active) {
        setDocumentRows({scope:documentScope,rows:[]});
        setChosenDocuments([]);
        setDocumentsError('Reference documents could not be loaded. Retry loading references.');
      }
    }).finally(() => { if (active) setDocumentsLoading(false); });
    return () => { active = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [documentScope, documentRevision]);
  useEffect(() => {
    let active = true;
    const read = ++sharingRead.current;
    setSharingError(sharingAdoptionError.current && sharingAdoptionError.current.ownerId === owner?.id ? sharingAdoptionError.current.error : '');
    sharingAdoptionError.current = null;
    setSharing({owner:ownerIdentity,enabled:false});
    setSharingLoading(!!owner);
    if (owner) void knowledgeService.documentSharing(owner).then(enabled => {
      if (active && read === sharingRead.current) setSharing({owner:ownerIdentity,enabled});
    }).catch(() => {
      if (active && read === sharingRead.current) setSharingError('Reference sharing could not be checked. Retry before asking with references.');
    }).finally(() => { if (active && read === sharingRead.current) setSharingLoading(false); });
    return () => { active = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ownerIdentity]);
  async function setDocumentSharing(enabled: boolean) {
    if (sharingPending.current || createPending.current || controller.pendingRequest()) return;
    if (!owner && (scope.kind !== 'library' || !scopeReady(scope))) return;
    sharingPending.current = true;
    sharingRead.current++;
    setSharingLoading(false);
    reset();
    setSharingSaving(true);
    setSharingError('');
    const current = controller.ticket('document-sharing');
    let requestOwner = owner;
    let createdOwner: ConversationOwner | null = null;
    try {
      if (!requestOwner) {
        createPending.current = true;
        setCreating(true);
        requestOwner = await knowledgeService.createConversation();
        if (!current()) return;
        createdOwner = requestOwner;
      }
      if (!current()) return;
      await knowledgeService.setDocumentSharing(requestOwner, enabled);
      if (!current()) return;
      setSharing({owner:JSON.stringify(requestOwner),enabled});
    } catch {
      if (current()) {
        const message = 'Reference sharing could not be saved. Try again.';
        setSharingError(message);
        if (createdOwner) sharingAdoptionError.current = {ownerId:createdOwner.id,error:message};
      }
    } finally {
      sharingPending.current = false;
      setSharingSaving(false);
      if (createdOwner) createPending.current = false;
      // A failed create also releases the synchronous creation guard.
      if (!owner) createPending.current = false;
      if (current()) {
        setCreating(false);
        if (createdOwner) {
          setThreads(rows => [createdOwner!, ...rows.filter(row => row.id !== createdOwner!.id)]);
          setOwner(createdOwner);
        }
      }
    }
  }
  async function search(query: string, mode: SearchMode) {
    reset();
    setResponse(null);
    if (!query.trim()) return;
    if (!scopeReady(scope)) {
      setError(
        "Select one or more meetings, or explicitly choose all saved meetings. Check the date range.",
      );
      return;
    }
    setLoading(true);
    const current = controller.ticket("search-operation");
    try {
      await controller.run(
        "search",
        () =>
          knowledgeService.search({
            scope,
            query: query.trim(),
            mode,
            document_ids: [...selectedDocumentIds],
          }),
        (value) => {
          setResponse(value);
          setLoading(false);
        },
      );
    } catch (e) {
      if (current()) {
        setError(errorText(e));
        setLoading(false);
      }
    }
  }
  async function newConversation() {
    if (createPending.current) return;
    createPending.current = true;
    reset();
    setCreating(true);
    const current = controller.ticket("new-conversation");
    try {
      const value = await knowledgeService.createConversation();
      if (!current()) return;
      setThreads((rows) => [value, ...rows]);
      setOwner(value);
    } catch (e) {
      if (current()) setError(errorText(e));
    } finally {
      createPending.current = false;
      if (current()) setCreating(false);
    }
  }
  async function ask(question: string, mode: SearchMode) {
    if (!question.trim() || historyLoading) return;
    if (controller.pendingRequest() || createPending.current || sharingPending.current) return;
    if (!scopeReady(scope)) {
      setError(
        "Select one or more meetings, or explicitly choose all saved meetings. Check the date range.",
      );
      return;
    }
    if (adoptedRetry.current?.question !== question.trim()) adoptedRetry.current = null;
    setSending(true);
    setError("");
    const current = controller.ticket("ask-operation");
    let createdOwner: ConversationOwner | null = null;
    let submissionError = "";
    let submittedId = "";
    try {
      await controller.submit(
        question.trim(),
        async (request_id) => {
          submittedId = request_id;
          let requestOwner = owner;
          if (!requestOwner && scope.kind === "library") {
            createPending.current = true;
            setCreating(true);
            try {
              requestOwner = await knowledgeService.createConversation();
            } finally {
              createPending.current = false;
              if (current()) setCreating(false);
            }
            if (!current()) return null;
            createdOwner = requestOwner;
          }
          if (!requestOwner || !current()) return null;
          await knowledgeService.ask({
            request_id,
            owner: requestOwner,
            search: { scope, query: question.trim(), mode, document_ids: [...selectedDocumentIds] },
          });
          if (!current()) return null;
          const history = await knowledgeService.history(requestOwner);
          if (!current()) return null;
          return history;
        },
        (history) => {
          // Hold the synchronous pending guard through durable history reconciliation.
          if (history) setMessages(history);
          adoptedRetry.current = null;
        },
        () => adoptedRetry.current?.id ?? globalThis.crypto.randomUUID(),
      );
    } catch (e) {
      if (current()) {
        submissionError = errorText(e);
        setError(submissionError);
      }
    } finally {
      if (current()) {
        setSending(false);
        // Adopt after reconciliation so an owner change cannot cancel its first turn.
        // Failed answers retain the durable owner and error for a retry in this thread.
        if (createdOwner) {
          const value: ConversationOwner = createdOwner;
          if (submissionError) {
            adoptionError.current = { ownerId: value.id, error: submissionError };
            adoptedRetry.current = {
              identity: JSON.stringify({ scope, owner: value, libraryRevision, answerConfiguration, selectedDocumentIds }),
              question: question.trim(),
              id: submittedId,
            };
          }
          setThreads((rows) => [value, ...rows.filter((row) => row.id !== value.id)]);
          setOwner(value);
        }
      }
    }
  }
  async function clear() {
    if (!owner) return;
    reset();
    setHistoryLoading(true);
    const current = controller.ticket("clear-operation");
    try {
      await knowledgeService.clear(owner);
      if (!current()) return;
      setMessages([]);
    } catch (e) {
      if (current()) setError(errorText(e));
    } finally {
      if (current()) setHistoryLoading(false);
    }
  }
  async function reloadHistory() {
    if (!owner) return;
    reset();
    setHistoryLoading(true);
    const current = controller.ticket("history-operation");
    try {
      await controller.run(
        "history",
        () => knowledgeService.history(owner),
        (value) => {
          setMessages(value);
          setHistoryLoading(false);
        },
      );
    } catch (e) {
      if (current()) {
        setError(errorText(e));
        setHistoryLoading(false);
      }
    }
  }
  async function reloadThreads() {
    const current = controller.ticket("threads-operation");
    try {
      await controller.run(
        "threads",
        knowledgeService.conversations,
        setThreads,
      );
    } catch (e) {
      if (current()) setError(errorText(e));
    }
  }
  async function inspect(
    reference: EvidenceRef,
    metadata: EvidenceDisplay,
    contextReference?: EvidenceRef,
  ) {
    setPreview({ reference, metadata, resolved: null, contextReference });
    setPreviewBusy(true);
    setError("");
    const current = controller.ticket("preview-operation");
    try {
      if (contextReference) {
        const origin = await knowledgeService.resolve(contextReference);
        if (!current()) return;
        if (origin.status !== "current") {
          setPreview({
            reference,
            metadata,
            contextReference,
            resolved: {
              status: origin.status,
              passage: null,
              navigation: null,
            },
          });
          return;
        }
      }
      const resolved = await knowledgeService.resolve(reference);
      if (!current()) return;
      setPreview({ reference, metadata, resolved, contextReference });
    } catch (e) {
      if (current()) setError(errorText(e));
    } finally {
      if (current()) setPreviewBusy(false);
    }
  }
  const visible = visibleFor === identity;
  return {
    scope,
    documents,
    selectedDocumentIds,
    documentsLoading: documentRows.scope !== documentScope || documentsLoading,
    documentsError,
    sharingEnabled,
    sharingLoading: sharingSaving || sharingLoading || (!!owner && sharing.owner !== ownerIdentity),
    sharingError,
    setDocumentSharing,
    reloadDocuments: () => { reset(); setDocumentRevision(value => value + 1); },
    selectDocument: (id: string, enabled: boolean) => {
      if (!documents.some(row => row.attachment.id === id)) return;
      reset();
      setChosenDocuments(ids => enabled ? [...new Set([...ids,id])] : ids.filter(value => value !== id));
    },
    owner,
    threads,
    creating,
    eventError,
    response: visible ? response : null,
    messages: visible ? messages : [],
    loading: visible && loading,
    sending: visible && sending,
    historyLoading: !visible || historyLoading,
    error: visible ? error : "",
    preview: visible ? preview : null,
    previewBusy: visible && previewBusy,
    controller,
    search,
    ask,
    clear,
    inspect,
    newConversation,
    reloadHistory,
    reloadThreads,
    cancel: reset,
    closePreview: () => {
      controller.ticket("preview-operation");
      setPreview(null);
      setPreviewBusy(false);
    },
    selectConversation: (value: ConversationOwner) => {
      reset();
      setOwner(value);
    },
  };
}
export type KnowledgeSearchState = ReturnType<typeof useKnowledgeSearch>;
