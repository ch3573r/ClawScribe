"use client";
import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { createKnowledgeController, scopeReady, selectedReferences } from "@/lib/knowledge-state";
import type { SavedReference } from "@/lib/archive-search";
import { knowledgeService, type NotePreview } from "@/services/knowledgeService";
import type {
  ConversationOwner,
  DocumentAttachment,
  EvidenceDisplay,
  EvidenceRef,
  HistoryMessage,
  KnowledgeScope,
  LibraryConversation,
  MeetingFilter,
  ResolvedEvidence,
  SearchMode,
  SearchResponse,
  ScopedDocument,
} from "@/types/knowledge";

// Draft scope is private to this app session. The last submitted scope is saved
// with its request and can be restored after restart without another store.
const conversationDrafts = new Map<string, MeetingFilter>();

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
  options?: { active?: boolean; draftMeetingIds?: string[]; restoreScope?: (filter: MeetingFilter | null) => void },
) {
  const surfaceActive = options?.active ?? true;
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
  const [threads, setThreads] = useState<LibraryConversation[]>([]);
  const threadRead = useRef(0);
  const historyMutation = useRef(false);
  const [managingConversation, setManagingConversation] = useState(false);
  const [threadsLoading, setThreadsLoading] = useState(false);
  const [threadsError, setThreadsError] = useState("");
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
  const [chosenDocuments, setChosenDocuments] = useState<{ owner: string; ids: string[]; imports?: DocumentAttachment[] }>({ owner: "", ids: [] });
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
  const chosen = chosenDocuments.owner === ownerIdentity ? chosenDocuments : null;
  const { files: selectedFiles, ids: selectedDocumentIds, verifying: verifyingImports } =
    selectedReferences(documents, chosen?.ids ?? [], chosen?.imports ?? []);
  const verifyingDocuments = verifyingImports || (!!chosen?.ids.length &&
    (documentRows.scope !== documentScope || documentsLoading || !!documentsError));
  const sharingEnabled = sharing.owner === ownerIdentity && sharing.enabled;
  const [creating, setCreating] = useState(false);
  const createPending = useRef(false);
  const adoptionError = useRef<{ ownerId: string; error: string } | null>(null);
  const adoptedRetry = useRef<{ identity: string; question: string; id: string } | null>(null);
  const scopeSwitchPending = useRef(false);
  const [preview, setPreview] = useState<{
    reference: EvidenceRef;
    metadata: EvidenceDisplay;
    resolved: ResolvedEvidence | null;
    contextReference?: EvidenceRef;
  } | null>(null);
  const [previewBusy, setPreviewBusy] = useState(false);
  const previewTrigger = useRef<HTMLElement | null>(null);
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
  const draftScope = scope.kind === "library" && owner?.kind === "library" && options?.restoreScope
    ? JSON.stringify({ ...scope.filter, meeting_ids: options.draftMeetingIds ?? scope.filter.meeting_ids })
    : null;
  useEffect(() => {
    if (!draftScope || !owner) return;
    conversationDrafts.delete(owner.id);
    conversationDrafts.set(owner.id, JSON.parse(draftScope) as MeetingFilter);
    if (conversationDrafts.size > 64) conversationDrafts.delete(conversationDrafts.keys().next().value!);
    // ownerIdentity is stable across equal owner objects.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draftScope, ownerIdentity]);

  const adoptOwner = (value: ConversationOwner) => {
    setChosenDocuments({ owner: JSON.stringify(value), ids: [...selectedDocumentIds] });
    setOwner(value);
  };
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
    if (!surfaceActive || !owner) {
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
  }, [identity, controller, surfaceActive]);
  const reloadThreads = useCallback(async () => {
    const sequence = ++threadRead.current;
    setThreadsLoading(true);
    setThreadsError("");
    try {
      const value = await knowledgeService.conversations();
      if (sequence === threadRead.current) setThreads(value);
    } catch (e) {
      if (sequence === threadRead.current) setThreadsError(errorText(e));
    } finally {
      if (sequence === threadRead.current) setThreadsLoading(false);
    }
  }, []);
  useEffect(() => {
    if (!surfaceActive || scope.kind !== "library") return;
    void reloadThreads();
    return () => { threadRead.current++; };
  }, [scope.kind, surfaceActive, reloadThreads]);
  useEffect(() => {
    if (!surfaceActive) return;
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
  }, [controller, surfaceActive]);
  // Scope reads have their own lifetime: changing a selected document must not
  // cancel a still-needed list or consent read and leave its loading state stuck.
  useEffect(() => {
    if (!surfaceActive) return;
    let active = true;
    setDocumentsError('');
    if (!scopeReady(scope)) {
      setDocumentRows({scope:documentScope,rows:[]});
      setChosenDocuments(current => ({ ...current, ids: [] }));
      setDocumentsLoading(false);
      return () => { active = false; };
    }
    setDocumentsLoading(true);
    void knowledgeService.scopeDocuments(scope).then(rows => {
      if (!active) return;
      setDocumentRows({scope:documentScope,rows});
      setChosenDocuments(current => {
        const found = new Set(rows.map(row => row.attachment.id));
        const imports = current.imports ?? [];
        return {
          owner: current.owner,
          ids: [...new Set([...current.ids, ...imports.map(file => file.id)].filter(id => found.has(id)))],
          imports: imports.filter(file => !found.has(file.id)),
        };
      });
    }).catch(() => {
      if (active) {
        setDocumentRows(current => current.scope === documentScope ? current : { scope: documentScope, rows: [] });
        setDocumentsError('Reference documents could not be loaded. Retry loading references.');
      }
    }).finally(() => { if (active) setDocumentsLoading(false); });
    return () => { active = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [documentScope, documentRevision, surfaceActive]);
  useEffect(() => {
    if (!surfaceActive) return;
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
  }, [ownerIdentity, surfaceActive]);
  async function setDocumentSharing(enabled: boolean) {
    if (!surfaceActive || scopeSwitchPending.current || historyMutation.current || sharingPending.current || createPending.current || controller.pendingRequest()) return;
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
          adoptOwner(createdOwner);
          void reloadThreads();
        }
      }
    }
  }
  async function search(query: string, mode: SearchMode) {
    if (!surfaceActive || scopeSwitchPending.current) return;
    reset();
    setResponse(null);
    if (!query.trim()) return;
    if (!scopeReady(scope)) {
      setError(
        "Choose meetings, a project or date filter, or select all saved meetings.",
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
    if (!surfaceActive || scopeSwitchPending.current || historyMutation.current || createPending.current) return;
    createPending.current = true;
    reset();
    setCreating(true);
    const current = controller.ticket("new-conversation");
    try {
      const value = await knowledgeService.createConversation();
      if (!current()) return;
      setThreads((rows) => [value, ...rows]);
      setChosenDocuments({ owner: JSON.stringify(value), ids: [] });
      setOwner(value);
      void reloadThreads();
    } catch (e) {
      if (current()) setError(errorText(e));
    } finally {
      createPending.current = false;
      if (current()) setCreating(false);
    }
  }
  async function ask(question: string, mode: SearchMode, review?: {
    references: SavedReference[];
    confirm: (preview: NotePreview) => Promise<boolean>;
    current: () => boolean;
  }) {
    if (!surfaceActive || scopeSwitchPending.current || !question.trim() || historyLoading || verifyingDocuments) return;
    if (controller.pendingRequest() || historyMutation.current || createPending.current || sharingPending.current) return;
    if (selectedDocumentIds.length && answerConfiguration?.provider !== "builtin-ai" &&
      (!sharingEnabled || sharingLoading || sharingSaving || sharing.owner !== ownerIdentity)) return;
    if (!scopeReady(scope)) {
      setError(
        "Choose meetings, a project or date filter, or select all saved meetings.",
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
    let answered = false;
    try {
      await controller.submit(
        review ? globalThis.crypto.randomUUID() : question.trim(),
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
          const request = {
            request_id,
            owner: requestOwner,
            search: { scope, query: question.trim(), mode, document_ids: [...selectedDocumentIds] },
          };
          if (review) {
            if (!review.current()) return null;
            const preview = await knowledgeService.previewNotes(request, review.references);
            try {
              if (!current() || !review.current() || !(await review.confirm(preview)) || !current() || !review.current()) {
                throw new Error("Question was not sent.");
              }
              await knowledgeService.askWithNotes(request, review.references, preview.token);
              answered = true;
            } finally {
              void knowledgeService.discardNoteReview(preview.token).catch(() => {});
            }
          } else {
            await knowledgeService.ask(request);
          }
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
        () => (!review && adoptedRetry.current?.id) || globalThis.crypto.randomUUID(),
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
            if (!review) adoptedRetry.current = {
              identity: JSON.stringify({ scope, owner: value, libraryRevision, answerConfiguration, selectedDocumentIds }),
              question: question.trim(),
              id: submittedId,
            };
          }
          setThreads((rows) => [value, ...rows.filter((row) => row.id !== value.id)]);
          adoptOwner(value);
          void reloadThreads();
        } else if (owner?.kind === "library") {
          void reloadThreads();
        }
      }
    }
    return answered;
  }
  async function clear(remove = false) {
    if (!owner || historyMutation.current || controller.pendingRequest()) return;
    historyMutation.current = true;
    setManagingConversation(true);
    reset();
    setHistoryLoading(true);
    const current = controller.ticket("clear-operation");
    try {
      if (remove) await knowledgeService.deleteConversation(owner);
      else await knowledgeService.clear(owner);
      if (owner.kind === "library") {
        conversationDrafts.delete(owner.id);
        setThreads(rows => rows.filter(row => row.id !== owner.id));
        setOwner(value => value?.id === owner.id ? null : value);
        void reloadThreads();
      }
      if (!current()) return;
      setMessages([]);
    } catch (e) {
      if (current()) setError(errorText(e));
    } finally {
      historyMutation.current = false;
      setManagingConversation(false);
      if (current()) setHistoryLoading(false);
    }
  }
  async function renameConversation(title: string) {
    if (owner?.kind !== "library" || historyMutation.current || controller.pendingRequest()) return;
    historyMutation.current = true;
    setManagingConversation(true);
    try {
      await knowledgeService.renameConversation(owner, title.trim());
      setThreads(rows => rows.map(row => row.id === owner.id ? { ...row, title: title.trim() } : row));
      void reloadThreads();
    } finally {
      historyMutation.current = false;
      setManagingConversation(false);
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
  const inspect = useCallback(async (
    reference: EvidenceRef,
    metadata: EvidenceDisplay,
    contextReference?: EvidenceRef,
    trigger?: HTMLElement,
  ) => {
    // Retry keeps the original trigger. A stable callback also keeps the
    // memoized Markdown from remounting the citation during verification.
    if (trigger) previewTrigger.current = trigger;
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
  }, [controller]);
  async function selectConversation({kind, id}: ConversationOwner) {
    const value = {kind, id};
    if (!surfaceActive || scopeSwitchPending.current || historyMutation.current || owner?.id === value.id) return;
    reset();
    if (!options?.restoreScope) {
      setChosenDocuments({ owner: JSON.stringify(value), ids: [] });
      setOwner(value);
      return;
    }
    scopeSwitchPending.current = true;
    setHistoryLoading(true);
    const current = controller.ticket("select-conversation");
    try {
      const draft = conversationDrafts.get(value.id);
      const saved = draft ? null : await knowledgeService.conversationScope(value);
      // Older IPC clients could combine all-meetings permission with IDs.
      // Preserve their active ID restriction when restoring that request.
      const filter = draft ?? (saved?.all_meetings && saved.meeting_ids.length ? { ...saved, all_meetings: false } : saved);
      if (!current()) return;
      options.restoreScope(filter);
      setChosenDocuments({ owner: JSON.stringify(value), ids: [] });
      setOwner(value);
    } catch (e) {
      if (current()) setError(errorText(e));
    } finally {
      scopeSwitchPending.current = false;
      if (current()) setHistoryLoading(false);
    }
  }
  const visible = surfaceActive && visibleFor === identity;
  return {
    scope,
    documents,
    selectedDocumentIds,
    selectedFiles,
    verifyingDocuments,
    documentsLoading: documentRows.scope !== documentScope || documentsLoading,
    documentsError,
    sharingEnabled,
    sharingLoading: sharingSaving || sharingLoading || (!!owner && sharing.owner !== ownerIdentity),
    sharingError,
    setDocumentSharing,
    reloadDocuments: () => { reset(); setDocumentRevision(value => value + 1); },
    refreshDocuments: () => setDocumentRevision(value => value + 1),
    selectImportedDocument: (attachment: DocumentAttachment) => {
      reset();
      setChosenDocuments(current => ({
        owner: ownerIdentity,
        ids: current.owner === ownerIdentity ? current.ids : [],
        imports: [...(current.owner === ownerIdentity ? current.imports ?? [] : [])
          .filter(file => file.id !== attachment.id), attachment],
      }));
      setDocumentRevision(value => value + 1);
    },
    selectDocument: (id: string, enabled: boolean) => {
      if (enabled && !documents.some(row => row.attachment.id === id)) return;
      reset();
      setChosenDocuments(current => {
        const ids = current.owner === ownerIdentity ? current.ids : [];
        const imports = current.owner === ownerIdentity ? current.imports : undefined;
        return {
          owner: ownerIdentity,
          ids: enabled ? [...new Set([...ids,id])] : ids.filter(value => value !== id),
          imports: enabled ? imports : imports?.filter(file => file.id !== id),
        };
      });
    },
    owner,
    threads,
    threadsLoading,
    threadsError,
    managingConversation,
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
    previewTrigger,
    controller,
    search,
    ask,
    clear,
    renameConversation,
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
    selectConversation,
  };
}
export type KnowledgeSearchState = ReturnType<typeof useKnowledgeSearch>;
