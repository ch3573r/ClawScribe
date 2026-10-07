"use client";
import { Children, useEffect, useRef, useState, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import { Info, MoreHorizontal } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { ScrollArea } from "@/components/ui/scroll-area";
import { PageSection } from "@/components/ui/page-section";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { citationParts, contextLinks, scopeReady, MAX_EVIDENCE_ENTRIES } from "@/lib/knowledge-state";
import { knowledgeService } from "@/services/knowledgeService";
import { openExternal } from "@/lib/openExternal";
import type { KnowledgeSearchState } from "@/hooks/useKnowledgeSearch";
import type { AssistantReply, HistoryMessage, SearchMode } from "@/types/knowledge";

function friendlyModel(model?: string) {
  if (!model) return "Choose a model";
  const gpt = /gpt[- ]?(\d+(?:\.\d+)?)/i.exec(model);
  if (gpt) return `GPT-${gpt[1]}`;
  const named = /^(qwen|llama|gemma|claude|gemini|deepseek)[-:]?([\d.]*)/i.exec(model.split("/").pop() ?? "");
  return named ? `${named[1][0].toUpperCase()}${named[1].slice(1)}${named[2] ? ` ${named[2]}` : ""}` : "Selected model";
}
function sourceTime(reply: AssistantReply, tag: number) {
  const locator = reply.evidence[tag - 1].locator;
  if (locator.kind !== "transcript" || locator.start_seconds == null) return "Time unavailable";
  const seconds = Math.max(0, Math.floor(locator.start_seconds));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}
function Citation({ reply, state, tag }: { reply: AssistantReply; state: KnowledgeSearchState; tag: number }) {
  return <Tooltip><TooltipTrigger asChild><Button type="button" variant="ghost" size="sm"
    className="mx-0.5 inline-flex h-5 min-w-5 align-super rounded px-1 text-xs text-primary"
    aria-label={`Open source ${tag}`} onClick={event => { event.preventDefault(); event.stopPropagation(); void state.inspect(reply.evidence[tag - 1], reply.evidence_metadata[tag - 1]); }}>{tag}</Button></TooltipTrigger>
    <TooltipContent>{reply.evidence_metadata[tag - 1].title} · {sourceTime(reply, tag)}</TooltipContent></Tooltip>;
}
function CitedAnswer({ reply, state }: { reply: AssistantReply; state: KnowledgeSearchState }) {
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
    a: ({children, href}) => <a href={href} className="text-primary underline" onClick={event => { event.preventDefault(); if (href) void openExternal(href); }}>{inline(children)}</a>,
    pre: ({children}) => <pre className="my-2 whitespace-pre-wrap rounded bg-background p-3">{children}</pre>,
  };
  return <TooltipProvider><div className="break-words text-sm leading-6"><ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>{reply.content}</ReactMarkdown></div>
    <div className="mt-2 flex items-center gap-2">
      {contextLinks(reply).map((link, index) => <Tooltip key={index}><TooltipTrigger asChild>
        <Button variant="ghost" size="sm" className="h-6 rounded-full bg-muted px-2 text-xs text-muted-foreground"
          aria-label={`Open question for source ${link.cited_tag}`} onClick={() => void state.inspect(reply.evidence[link.context_tag - 1], reply.evidence_metadata[link.context_tag - 1], reply.evidence[link.cited_tag - 1])}>question</Button>
      </TooltipTrigger><TooltipContent>{reply.evidence_metadata[link.context_tag - 1].title} · {sourceTime(reply, link.context_tag)}</TooltipContent></Tooltip>)}
      <Tooltip><TooltipTrigger asChild><Button variant="ghost" size="icon" className="h-6 w-6 text-muted-foreground" aria-label="Answer details"><Info className="h-3.5 w-3.5" /></Button></TooltipTrigger>
        <TooltipContent>Answered with {friendlyModel(reply.model)} · {reply.provider} · {reply.retrieval_mode === "hybrid" ? "Semantic + keyword" : "Keyword"} search</TooltipContent></Tooltip>
    </div>
  </TooltipProvider>;
}
function conversationName(messages: HistoryMessage[]) {
  const first = messages.find(message => message.role === "user" && message.content.trim());
  if (!first) return "New conversation";
  const date = new Date(first.created_at);
  const label = first.content.replace(/\s+/g, " ").trim().slice(0, 60);
  return `${label}${Number.isNaN(date.getTime()) ? "" : ` · ${date.toLocaleDateString(undefined, { month: "short", day: "numeric" })}`}`;
}
export function KnowledgeChat({ state, mode, provider, model, title = "Ask about these meetings", secondary }: {
  state: KnowledgeSearchState; mode: SearchMode; provider?: string; model?: string; title?: string; secondary?: ReactNode;
}) {
  const [question, setQuestion] = useState("");
  const [menuOpen, setMenuOpen] = useState(false);
  const [names, setNames] = useState<Record<string, string>>({});
  const [namesLoading, setNamesLoading] = useState(false);
  const [namesError, setNamesError] = useState(false);
  const [namesRetry, setNamesRetry] = useState(0);
  const [settled, setSettled] = useState(0);
  const end = useRef<HTMLDivElement>(null);
  const pending = useRef<{ text: string; scope: string; owner?: string; provider?: string; model?: string; started: boolean; finished: boolean } | null>(null);
  const scope = JSON.stringify(state.scope);
  const ready = !!provider && !!model && (!!state.owner || state.scope.kind === "library") && scopeReady(state.scope) && !state.historyLoading && !state.creating;
  useEffect(() => { end.current?.scrollIntoView({ block: "end", behavior: "smooth" }); }, [state.messages, state.sending]);
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
  useEffect(() => {
    if (!menuOpen || state.scope.kind !== "library") return;
    let active = true;
    setNamesLoading(true); setNamesError(false);
    void (async () => {
      for (const owner of state.threads) {
        if (!active) return;
        if (owner.id === state.owner?.id || names[owner.id]) continue;
        try {
          const history = await knowledgeService.history(owner);
          if (active) setNames(current => ({ ...current, [owner.id]: conversationName(history) }));
        } catch { if (active) setNamesError(true); }
      }
      if (active) setNamesLoading(false);
    })();
    return () => { active = false; };
    // Cached names do not restart an in-flight menu read.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [menuOpen, state.threads, state.owner?.id, namesRetry, state.scope.kind]);
  const submit = () => {
    if (!question.trim() || !ready || state.sending || pending.current) return;
    const request = { text: question, scope, owner: state.owner?.id, provider, model, started: false, finished: false };
    pending.current = request; setQuestion("");
    void state.ask(request.text, mode).catch(() => { if (pending.current === request) setQuestion(current => current || request.text); }).finally(() => {
      request.finished = true; if (pending.current === request) setSettled(current => current + 1);
    });
  };
  return <PageSection className="flex h-full min-h-0 flex-col overflow-hidden" aria-label={title}>
    <header className="flex shrink-0 items-center gap-3 border-b border-border px-5 py-3">
      <h2 className="min-w-0 flex-1 font-semibold">{title}</h2>
      <span className="rounded-full bg-muted px-2 py-1 text-xs text-muted-foreground">{friendlyModel(model)}</span>
      <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen}><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" aria-label="Conversations" disabled={state.sending || state.creating || state.historyLoading}><MoreHorizontal /></Button></DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="max-w-sm">
          <DropdownMenuLabel>Conversations</DropdownMenuLabel>
          {state.scope.kind === "library" && <DropdownMenuItem onSelect={() => void state.newConversation()}>New conversation</DropdownMenuItem>}
          <DropdownMenuItem disabled={!state.messages.length || !state.owner} onSelect={() => { if (state.owner) setNames(current => { const next = { ...current }; delete next[state.owner!.id]; return next; }); void state.clear(); }}>Clear conversation</DropdownMenuItem>
          {state.scope.kind === "library" && <><DropdownMenuSeparator />
            {state.threads.map(owner => <DropdownMenuItem key={owner.id} className="max-w-xs truncate" onSelect={() => state.selectConversation(owner)}>{owner.id === state.owner?.id ? conversationName(state.messages) : names[owner.id] ?? "Loading conversation…"}</DropdownMenuItem>)}
            {!state.threads.length && <DropdownMenuLabel>No saved conversations</DropdownMenuLabel>}
            {namesLoading && <DropdownMenuLabel>Loading names…</DropdownMenuLabel>}
            {namesError && <DropdownMenuItem onSelect={event => { event.preventDefault(); setNamesRetry(value => value + 1); }}>Retry loading names</DropdownMenuItem>}
          </>}
        </DropdownMenuContent>
      </DropdownMenu>
    </header>
    <ScrollArea className="min-h-0 flex-1">
      <div className="space-y-4 p-5" aria-live="polite">
        {secondary}
        {state.historyLoading && <p role="status" className="text-sm text-muted-foreground">Loading conversation…</p>}
        {state.creating && !state.sending && <p role="status" className="text-sm text-muted-foreground">Creating conversation…</p>}
        {!state.messages.length && !state.historyLoading && <p className="text-sm text-muted-foreground">Ask about decisions, action items, or what was said.</p>}
        {state.messages.map(message => <article key={message.id} className={`rounded-lg p-3 ${message.role === "user" ? "ml-6 bg-primary/10" : "bg-muted"}`}>
          <p className="mb-1 text-xs font-medium text-muted-foreground">{message.role === "user" ? "You" : "Assistant"}{message.status !== "completed" ? ` · ${message.status === "failed" ? "Failed" : message.status === "cancelled" ? "Cancelled" : "Pending"}` : ""}</p>
          {message.reply ? <CitedAnswer reply={message.reply} state={state} /> : <p className="whitespace-pre-wrap break-words text-sm">{message.content}</p>}
        </article>)}
        {state.sending && <p role="status" className="text-sm text-muted-foreground">Finding sources and answering…</p>}
        {state.error && <p role="alert" className="text-sm text-destructive">{state.error}</p>}
        {state.eventError && <p role="status" className="text-xs text-muted-foreground">{state.eventError}</p>}
        <div ref={end} />
      </div>
    </ScrollArea>
    <form className="sticky bottom-0 shrink-0 space-y-3 border-t border-border bg-card p-5" onSubmit={event => { event.preventDefault(); submit(); }}>
      <label className="block text-xs font-medium" htmlFor={`knowledge-question-${state.owner?.id ?? "library"}`}>Question</label>
      <Textarea id={`knowledge-question-${state.owner?.id ?? "library"}`} value={question} onChange={event => setQuestion(event.target.value)} rows={2} placeholder="What did we decide?" className="min-h-16 resize-none"
        onKeyDown={event => { if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent?.isComposing) { event.preventDefault(); submit(); } }} />
      <div className="flex items-center gap-2"><Button type="submit" disabled={!ready || state.sending || !question.trim()}>Ask</Button>
        {state.sending && <Button type="button" variant="outline" onClick={state.cancel}>Cancel answer</Button>}
        <span className="text-xs text-muted-foreground">Shift+Enter for a new line</span>
      </div>
      {!provider || !model ? <p className="text-xs text-muted-foreground">Choose a summary provider and model in Settings.</p> : null}
    </form>
  </PageSection>;
}
