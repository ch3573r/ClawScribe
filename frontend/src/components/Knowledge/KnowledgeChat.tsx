"use client";
import { Children, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { Copy, Info, MoreHorizontal } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { ScrollArea } from "@/components/ui/scroll-area";
import { PageSection } from "@/components/ui/page-section";
import type { SavedReference } from "@/lib/archive-search";
import type { NotePreview } from "@/services/knowledgeService";
import { ConfirmationModal } from "@/components/ConfirmationModel/confirmation-modal";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { citationParts, contextLinks, scopeReady, MAX_EVIDENCE_ENTRIES } from "@/lib/knowledge-state";
import { documentAnchor } from "@/lib/knowledge-documents";
import { friendlyModel } from "@/lib/friendly-model";
import { openExternal } from "@/lib/openExternal";
import type { KnowledgeSearchState } from "@/hooks/useKnowledgeSearch";
import type { AssistantReply, ConversationOwner, HistoryMessage, SearchMode } from "@/types/knowledge";

function sourceTime(reply: AssistantReply, tag: number) {
  const locator = reply.evidence[tag - 1].locator;
  if (locator.kind === "document") return documentAnchor(locator);
  if (locator.kind === "live") return "Current recording";
  if (locator.kind !== "transcript" || locator.start_seconds == null) return "Time unavailable";
  const seconds = Math.max(0, Math.floor(locator.start_seconds));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}
function Citation({ reply, state, tag }: { reply: AssistantReply; state: Pick<KnowledgeSearchState, 'inspect'>; tag: number }) {
  return <Tooltip><TooltipTrigger asChild><Button type="button" variant="ghost" size="sm"
    className="mx-0.5 inline-flex h-5 min-w-5 align-super rounded px-1 text-xs text-primary"
    aria-label={`Open source ${tag}`} onClick={event => { event.preventDefault(); event.stopPropagation(); void state.inspect(reply.evidence[tag - 1], reply.evidence_metadata[tag - 1], undefined, event.currentTarget); }}>{reply.evidence[tag - 1].locator.kind === "document" ? sourceTime(reply, tag) : tag}</Button></TooltipTrigger>
    <TooltipContent>{reply.evidence_metadata[tag - 1].title} · {sourceTime(reply, tag)}</TooltipContent></Tooltip>;
}
export function CitedAnswer({ reply, state }: { reply: AssistantReply; state: Pick<KnowledgeSearchState, 'inspect'> }) {
  const count = reply.evidence.length === reply.evidence_metadata.length && reply.evidence.length <= MAX_EVIDENCE_ENTRIES ? reply.evidence.length : 0;
  const inline = (children: ReactNode) => Children.map(children, child => typeof child === "string"
    ? citationParts(child, count, reply.cited_tags).map((part, index) => part.tags
      ? part.tags.map((tag, n) => <Citation key={`${index}-${tag}-${n}`} reply={reply} state={state} tag={tag} />)
      : part.text.replace(/\[[^\]]*\bK\d+[^\]]*\]/g, "(source unavailable)")) : child);
  const components: Components = {
    p: ({children}) => <p className="my-2">{inline(children)}</p>,
    li: ({children}) => <li className="my-1">{inline(children)}</li>,
    strong: ({children}) => <strong>{inline(children)}</strong>,
    em: ({children}) => <em>{inline(children)}</em>,
    del: ({children}) => <del>{inline(children)}</del>,
    h1: ({children}) => <h3 className="my-3 font-semibold">{inline(children)}</h3>,
    h2: ({children}) => <h3 className="my-3 font-semibold">{inline(children)}</h3>,
    h3: ({children}) => <h3 className="my-3 font-semibold">{inline(children)}</h3>,
    h4: ({children}) => <h4 className="my-2 font-semibold">{inline(children)}</h4>,
    h5: ({children}) => <h4 className="my-2 font-semibold">{inline(children)}</h4>,
    h6: ({children}) => <h4 className="my-2 font-semibold">{inline(children)}</h4>,
    ul: ({children}) => <ul className="my-2 list-disc space-y-1 pl-5">{children}</ul>,
    ol: ({children}) => <ol className="my-2 list-decimal space-y-1 pl-5">{children}</ol>,
    blockquote: ({children}) => <blockquote className="border-l-2 border-border pl-3">{inline(children)}</blockquote>,
    table: ({children}) => <table className="my-2 w-full table-fixed border-collapse">{children}</table>,
    th: ({children}) => <th className="border border-border p-2 text-left">{inline(children)}</th>,
    td: ({children}) => <td className="border border-border p-2">{inline(children)}</td>,
    a: ({children, href}) => <Button variant="link" asChild className="inline h-auto whitespace-normal p-0"><a href={href} onClick={event => { event.preventDefault(); if (href) void openExternal(href); }}>{inline(children)}</a></Button>,
    pre: ({children}) => <pre className="my-2 whitespace-pre-wrap rounded bg-background p-3">{children}</pre>,
  };
  return <TooltipProvider><div className="break-words text-sm leading-6"><ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>{reply.content}</ReactMarkdown></div>
    <div className="mt-2 flex flex-wrap items-center gap-2">
      {contextLinks(reply).map((link, index) => <Tooltip key={index}><TooltipTrigger asChild>
        <Button variant="ghost" size="sm" className="h-6 rounded-full bg-muted px-2 text-xs text-muted-foreground"
          aria-label={`Open question for source ${link.cited_tag}`} onClick={event => void state.inspect(reply.evidence[link.context_tag - 1], reply.evidence_metadata[link.context_tag - 1], reply.evidence[link.cited_tag - 1], event.currentTarget)}>question</Button>
      </TooltipTrigger><TooltipContent>{reply.evidence_metadata[link.context_tag - 1].title} · {sourceTime(reply, link.context_tag)}</TooltipContent></Tooltip>)}
      <Tooltip><TooltipTrigger asChild><Button variant="ghost" size="icon" className="h-6 w-6 text-muted-foreground" aria-label="Answer details"><Info className="h-3.5 w-3.5" /></Button></TooltipTrigger>
        <TooltipContent>Answered with {reply.model || friendlyModel(reply.model)} · {reply.provider} · {reply.retrieval_mode === "hybrid" ? "Semantic + keyword" : "Keyword"} search</TooltipContent></Tooltip>
    </div>
  </TooltipProvider>;
}
function conversationName(messages: HistoryMessage[]) {
  const first = messages.find(message => message.role === "user" && message.content.trim());
  if (!first) return "New conversation";
  return conversationLabel(first.content, first.created_at);
}
function conversationLabel(question?: string | null, createdAt?: string) {
  if (!question?.trim()) return "New conversation";
  const date = new Date(createdAt ?? "");
  const label = question.replace(/\s+/g, " ").trim().slice(0, 60);
  return `${label}${Number.isNaN(date.getTime()) ? "" : ` · ${date.toLocaleDateString(undefined, { month: "short", day: "numeric" })}`}`;
}
export function KnowledgeChat({ state, mode, provider, model, title = "Ask about these meetings", secondary, compact = false, headerActions, contextBar, composerStart, attachments, suggestions, headerMenuExtra, emptyState, initialQuestion, selectedNotes, removeNote, clearNotes, reviewActive = true, destinationRevision }: {
  state: KnowledgeSearchState; mode: SearchMode; provider?: string; model?: string; title?: string; secondary?: ReactNode; compact?: boolean; headerActions?: ReactNode;
  contextBar?: ReactNode; composerStart?: ReactNode; attachments?: ReactNode; suggestions?: readonly string[]; headerMenuExtra?: ReactNode;
  emptyState?: ReactNode;
  initialQuestion?: string;
  selectedNotes?: SavedReference[]; removeNote?: (reference: SavedReference) => void; clearNotes?: () => void;
  reviewActive?: boolean; destinationRevision?: string;
}) {
  const [question, setQuestion] = useState(initialQuestion ?? "");
  const [menuOpen, setMenuOpen] = useState(false);
  const [clearTarget, setClearTarget] = useState<ConversationOwner | null>(null);
  useEffect(() => setClearTarget(null), [state.owner?.kind, state.owner?.id]);
  const [settled, setSettled] = useState(0);
  const end = useRef<HTMLDivElement>(null);
  const menuTrigger = useRef<HTMLButtonElement>(null);
  const busy = state.sending || state.creating || state.historyLoading;
  const questionRef = useRef<HTMLTextAreaElement>(null);
  const pending = useRef<{ text: string; scope: string; owner?: string; provider?: string; model?: string; started: boolean; finished: boolean } | null>(null);
  const scope = JSON.stringify({scope:state.scope,documents:state.selectedDocumentIds});
  const noteKey = JSON.stringify([selectedNotes, question, scope, state.owner?.id, mode, provider, model, destinationRevision, reviewActive]);
  const currentNoteKey = useRef(noteKey);
  currentNoteKey.current = noteKey;
  const mounted = useRef(true);
  const waiting = useRef<{ token: string; resolve: (answer: boolean) => void } | null>(null);
  const [review, setReview] = useState<NotePreview | null>(null);
  const closeReview = (confirmed = false, token?: string) => {
    const entry = waiting.current;
    if (token && entry?.token !== token) return;
    waiting.current = null;
    setReview(null);
    entry?.resolve(confirmed);
  };
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; waiting.current?.resolve(false); waiting.current = null; };
  }, []);
  useEffect(() => { closeReview(); }, [noteKey]);
  const hasNotes = !!selectedNotes?.length;
  const notesInScope = !hasNotes || (state.scope.kind === "library" && !state.scope.filter.all_meetings
    && selectedNotes.every(ref => state.scope.kind === "library" && state.scope.filter.meeting_ids.includes(ref.meeting_id)));
  const externalReferences = !!state.selectedDocumentIds?.length && provider !== "builtin-ai";
  const needsSharing = externalReferences && !state.sharingEnabled;
  const ready = notesInScope && !!provider && !!model && (!!state.owner || state.scope.kind === "library") && scopeReady(state.scope) && !state.historyLoading && !state.creating && !needsSharing && (!externalReferences || !state.sharingLoading);
  // Draft keystrokes must not rebuild the Markdown/citation tree for the entire history.
  const messages = useMemo(() => state.messages.map(message => <article key={message.id} className={`rounded-lg p-3 ${message.role === "user" ? "ml-6 bg-primary/10" : "bg-muted"}`}>
    <div className="mb-1 flex items-center justify-between gap-2">
      <p className="text-xs font-medium text-muted-foreground">{message.role === "user" ? "You" : "Assistant"}{message.status !== "completed" ? ` · ${message.status === "failed" ? "Failed" : message.status === "cancelled" ? "Cancelled" : "Pending"}` : ""}</p>
      {message.role === "assistant" && (message.status === "completed" || message.legacy) && message.content.trim() && <Button type="button" variant="ghost" size="icon" className="h-7 w-7" aria-label="Copy answer" title="Copy answer" onClick={async () => {
        try {
          await globalThis.navigator.clipboard.writeText(message.content);
          toast.success("Answer copied");
        } catch {
          toast.error("Could not copy the answer");
        }
      }}><Copy /></Button>}
    </div>
    {message.reply ? <CitedAnswer reply={message.reply} state={{inspect:state.inspect}} /> : <p className="whitespace-pre-wrap break-words text-sm">{message.content}</p>}
  </article>), [state.messages, state.inspect]);
  useEffect(() => {
    if (state.messages.length || state.sending) end.current?.scrollIntoView({ block: "end", behavior: "smooth" });
  }, [state.messages, state.sending]);
  useEffect(() => {
    const request = pending.current;
    if (!request) return;
    if (request.scope !== scope || request.provider !== provider || request.model !== model || (request.owner && request.owner !== state.owner?.id)) { pending.current = null; return; }
    if (state.sending || state.creating) { request.started = true; return; }
    if (!request.started && !request.finished) return;
    const last = state.messages.at(-1);
    if (state.error || last?.status === "failed" || last?.status === "cancelled") setQuestion(current => current || request.text);
    pending.current = null;
  }, [state.sending, state.creating, state.error, state.messages, scope, state.owner?.id, provider, model, settled]);
  const submit = () => {
    if (!question.trim() || !ready || state.sending || pending.current) return;
    const request = { text: question, scope, owner: state.owner?.id, provider, model, started: false, finished: false };
    pending.current = request;
    const current = () => mounted.current && pending.current === request && currentNoteKey.current === noteKey && reviewActive;
    const reviewNotes = selectedNotes?.length ? {
      references: selectedNotes,
      current,
      confirm: (preview: NotePreview) => new Promise<boolean>(resolve => {
        if (!current()) { resolve(false); return; }
        waiting.current = { token: preview.token, resolve };
        setReview(preview);
      }),
    } : undefined;
    if (!reviewNotes) setQuestion("");
    const operation = reviewNotes ? state.ask(request.text, mode, reviewNotes) : state.ask(request.text, mode);
    void operation.then(answered => {
      if (reviewNotes && answered && currentNoteKey.current === noteKey) {
        clearNotes?.();
        setQuestion(value => value === request.text ? "" : value);
      }
    }).catch(() => { if (pending.current === request) setQuestion(current => current || request.text); }).finally(() => {
      request.finished = true;
      if (pending.current === request) setSettled(current => current + 1);
    });
  };
  const questionInput = <Textarea ref={questionRef} id={`knowledge-question-${state.owner?.id ?? "library"}`} value={question} disabled={hasNotes && busy} onChange={event => setQuestion(event.target.value)} rows={1} placeholder={compact ? "Ask about this meeting…" : "Ask about decisions, action items or what was said"} className={`${compact ? "min-h-10" : "min-h-11"} resize-none`}
    onKeyDown={event => { if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent?.isComposing) { event.preventDefault(); submit(); } }} />;
  return <PageSection className={`flex h-full min-h-0 flex-col overflow-hidden ${compact ? "" : "meeting-memory-chat custom-scrollbar"}`} aria-label={title}>
    <header className={`flex shrink-0 items-center gap-2 border-b border-border py-3 ${compact ? "px-3" : "px-5"}`}>
      <h2 className={`min-w-0 flex-1 font-semibold ${compact ? "text-sm" : ""}`}>{title}</h2>
      <TooltipProvider><Tooltip><TooltipTrigger asChild><span tabIndex={0} className="max-w-48 truncate rounded-full bg-muted px-2 py-1 text-xs text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">{friendlyModel(model)}</span></TooltipTrigger><TooltipContent>{model || "Choose a model"}</TooltipContent></Tooltip></TooltipProvider>
      <DropdownMenu open={menuOpen} onOpenChange={open => { setMenuOpen(open); if (open && state.scope.kind === "library") void state.reloadThreads(); }}><DropdownMenuTrigger asChild><Button ref={menuTrigger} variant="ghost" size="icon" aria-label={compact ? "More options" : "Conversations"} disabled={busy}><MoreHorizontal /></Button></DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="max-w-sm">
          {compact && headerMenuExtra}
          <DropdownMenuLabel>Conversations</DropdownMenuLabel>
          {state.scope.kind === "library" && <DropdownMenuItem onSelect={() => void state.newConversation()}>New conversation</DropdownMenuItem>}
          <DropdownMenuItem disabled={!state.messages.length || !state.owner || busy} onSelect={() => setClearTarget(state.owner)}>Clear conversation</DropdownMenuItem>
          {state.scope.kind === "library" && <><DropdownMenuSeparator />
            {state.threads.map(owner => <DropdownMenuItem key={owner.id} className="max-w-xs truncate" onSelect={() => state.selectConversation(owner)}>{owner.id === state.owner?.id ? conversationName(state.messages) : conversationLabel(owner.first_question, owner.created_at)}</DropdownMenuItem>)}
            {!state.threads.length && <DropdownMenuLabel>No saved conversations</DropdownMenuLabel>}
          </>}
        </DropdownMenuContent>
      </DropdownMenu>
      {headerActions}
    </header>
    {clearTarget && clearTarget.kind === state.owner?.kind && clearTarget.id === state.owner.id &&
      <ConfirmationModal isOpen title="Clear conversation?" confirmLabel="Clear conversation"
        text={`Remove all messages and saved citations from “${conversationName(state.messages)}”?${busy ? " Wait for the current answer to finish." : ""}`}
        returnFocus={menuTrigger} confirmDisabled={busy}
        onCancel={() => setClearTarget(null)}
        onConfirm={() => {
          // Stay open instead of closing with nothing done.
          if (busy) return;
          setClearTarget(null);
          void state.clear();
        }}
      />}
    {review && <ConfirmationModal isOpen title="Review personal excerpts" text="Confirm the exact text and destination for this question."
      confirmLabel="Confirm and send" confirmVariant="default" onCancel={() => closeReview()} onConfirm={() => closeReview(true, review.token)}>
      <div className="max-h-[55vh] space-y-3 overflow-y-auto">
      <p className="whitespace-pre-wrap break-words text-sm"><strong>Question</strong><br />{review.question}</p>
      <p className="break-all text-sm"><strong>Destination</strong><br />{review.destination.provider} · {review.destination.model}<br />{review.destination.endpoint ?? "Built-in AI on this device"}
        {review.destination.organization && <> · Organization {review.destination.organization}</>}{review.destination.project && <> · Project {review.destination.project}</>}</p>
      <ol className="space-y-2">{review.excerpts.map((excerpt, index) => <li key={index} className="rounded border border-border p-2">
        <div className="flex items-center justify-between gap-2"><strong className="text-xs">{excerpt.source === "note" ? "Saved note" : "Bookmark"}</strong>
          <Button variant="ghost" size="sm" onClick={() => { closeReview(); const ref = selectedNotes?.[index]; if (ref) removeNote?.(ref); }}>Remove</Button></div>
        <p className="whitespace-pre-wrap break-words text-sm">{excerpt.text}</p>
      </li>)}</ol>
      <p className="text-xs text-muted-foreground">The question also uses transcript evidence, available summary overviews and any selected references from these meetings. Personal excerpts apply only to this question.</p>
      </div>
    </ConfirmationModal>}
    {compact && contextBar && <div className="shrink-0 border-b border-border px-3 py-1">{contextBar}</div>}
    <ScrollArea className={`min-h-0 flex-1 [&_[data-radix-scroll-area-viewport]>div]:!block ${compact ? "" : "meeting-memory-conversation"} ${emptyState ? "[&_[data-radix-scroll-area-viewport]>div]:h-full" : ""}`}>
      <div className={`space-y-4 ${compact ? "p-3" : "p-5"} ${emptyState ? "flex h-full flex-col" : ""}`}>
        {!compact && secondary}
        <div className={`space-y-4 ${emptyState ? "flex flex-1 flex-col" : ""}`} aria-live="polite">
          {state.historyLoading && <p role="status" className="text-sm text-muted-foreground">Loading conversation…</p>}
          {state.creating && !state.sending && <p role="status" className="text-sm text-muted-foreground">Creating conversation…</p>}
          {emptyState ? <div className="flex flex-1 items-center justify-center py-12">{emptyState}</div> : !state.messages.length && !state.historyLoading && (compact && suggestions?.length ? <div className="flex flex-col items-start gap-2" aria-label="Suggested questions">
            {suggestions.slice(0, 3).map(suggestion => <Button key={suggestion} type="button" variant="outline" size="sm" onClick={() => { setQuestion(suggestion); questionRef.current?.focus(); }}>{suggestion}</Button>)}
          </div> : <p className="text-sm text-muted-foreground">Ask about decisions, action items, or what was said.</p>)}
          {!emptyState && messages}
          {state.sending && <p role="status" className="text-sm text-muted-foreground">Finding sources and answering…</p>}
          {state.error && <p role="alert" className="text-sm text-destructive">{state.error}</p>}
          {state.eventError && <p role="status" className="text-xs text-muted-foreground">{state.eventError}</p>}
          <div ref={end} />
        </div>
      </div>
    </ScrollArea>
    {hasNotes && <div className="flex shrink-0 items-center gap-2 px-5 text-xs text-muted-foreground">
      <span>{notesInScope ? "Review selected excerpts before asking." : "A selected excerpt is outside the current meeting selection."}</span>
      <Button type="button" size="sm" variant="ghost" disabled={busy} onClick={clearNotes}>Remove all excerpts</Button></div>}
    <form className={`shrink-0 border-t border-border bg-card ${compact ? "sticky bottom-0 space-y-2 p-3" : "space-y-3 p-5"}`} onSubmit={event => { event.preventDefault(); submit(); }}>
      <label className={compact ? "sr-only" : "block text-xs font-medium"} htmlFor={`knowledge-question-${state.owner?.id ?? "library"}`}>Question</label>
      {compact && attachments}
      {compact ? <div className="flex flex-wrap items-end gap-2">{composerStart}<div className="min-w-0 flex-1">{questionInput}</div></div> : <div className="flex items-end gap-3"><div className="min-w-0 flex-1">{questionInput}</div><Button type="submit" disabled={!ready || state.sending || !question.trim()}>{hasNotes ? "Review and ask" : "Ask"}</Button></div>}
      <div className="flex items-center gap-2">{compact && <Button type="submit" disabled={!ready || state.sending || !question.trim()}>Ask</Button>}
        {state.sending && <Button type="button" variant="outline" onClick={() => { closeReview(); state.cancel(); }}>Cancel answer</Button>}
        <span className="text-xs text-muted-foreground">Shift+Enter for a new line</span>
      </div>
      {!provider || !model ? <p className="text-xs text-muted-foreground">Choose a summary provider and model in Settings.</p> : null}
      {needsSharing && <p className="text-xs text-muted-foreground">{compact ? "Enable reference sharing in Context, or remove files to ask about transcripts only." : "Enable reference sharing above, or deselect references to ask about transcripts only."}</p>}
    </form>
  </PageSection>;
}
