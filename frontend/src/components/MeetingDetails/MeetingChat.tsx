import React, { useEffect, useId, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { invoke } from '@tauri-apps/api/core';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { MessageSquare, X, Send, Loader2, Trash2 } from 'lucide-react';
import { MeetingChatSession } from '@/lib/meetingChatSession';
import { ConfirmationModal } from '../ConfirmationModel/confirmation-modal';

interface MeetingChatProps {
  meetingId: string;
  provider?: string;
  model?: string;
}

/** Key the stateful panel so changing meetings cannot expose the old draft/history. */
export function MeetingChat(props: MeetingChatProps) {
  return <MeetingChatPanel key={props.meetingId} {...props} />;
}

function MeetingChatPanel({ meetingId, provider, model }: MeetingChatProps) {
  const session = useMemo(() => new MeetingChatSession(meetingId, invoke), [meetingId]);
  const state = useSyncExternalStore(session.subscribe, session.getSnapshot, session.getSnapshot);
  const [open, setOpen] = useState(false);
  const [confirmClear, setConfirmClear] = useState(false);
  const panelId = useId();
  const titleId = useId();
  const inputId = useId();
  const launcherRef = useRef<HTMLButtonElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const followRef = useRef(true);
  const canSend = Boolean(provider && model) && state.loaded && !state.busy && state.draft.trim().length > 0;

  useEffect(() => () => session.invalidate(), [session]);
  useEffect(() => { if (open) void session.load(); }, [open, session]);
  useEffect(() => {
    if (open && state.loaded && !state.busy) inputRef.current?.focus();
  }, [open, state.loaded]);
  useEffect(() => {
    const panel = scrollRef.current;
    // Never force someone reading earlier answers back to the bottom.
    if (panel && followRef.current) panel.scrollTop = panel.scrollHeight;
  }, [state.messages, state.busy]);

  const close = () => {
    setOpen(false);
    launcherRef.current?.focus();
  };

  return (
    <div className="fixed bottom-3 right-3 z-40 sm:bottom-6 sm:right-6">
      {open && (
        <section
          id={panelId}
          aria-labelledby={titleId}
          onKeyDown={event => { if (event.key === 'Escape') { event.stopPropagation(); close(); } }}
          className="mb-3 flex h-[32rem] max-h-[calc(100dvh-6rem)] w-[26rem] max-w-[calc(100vw-1.5rem)] flex-col overflow-hidden rounded-xl border border-border bg-card shadow-2xl"
        >
          <div className="flex shrink-0 items-center justify-between gap-2 border-b border-border px-4 py-3">
            <h2 id={titleId} className="flex min-w-0 items-center gap-2 text-sm font-semibold text-foreground">
              <MessageSquare aria-hidden="true" className="h-4 w-4 shrink-0 text-primary" />
              Chat with this meeting
            </h2>
            <div className="flex shrink-0 items-center gap-1">
              {state.messages.length > 0 && (
                <button type="button" onClick={() => setConfirmClear(true)}
                  disabled={Boolean(state.busy)} aria-label="Clear chat history"
                  className="rounded p-2 text-muted-foreground hover:bg-destructive/10 hover:text-destructive focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-40">
                  <Trash2 aria-hidden="true" className="h-4 w-4" />
                </button>
              )}
              <button type="button" onClick={close} aria-label="Close meeting chat"
                className="rounded p-2 text-muted-foreground hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
                <X aria-hidden="true" className="h-4 w-4" />
              </button>
            </div>
          </div>

          <p className="shrink-0 border-b border-border px-4 py-2 text-xs leading-5 text-muted-foreground">
            {state.contextTruncated
              ? 'This answer used only the beginning and end of the transcript. The middle was omitted; check the original before relying on it.'
              : 'AI answers can be wrong. Long transcripts may be shortened to the beginning and end. Verify important details against the transcript.'}
          </p>
          <div ref={scrollRef} role="log" aria-label="Meeting chat messages" aria-live="polite" aria-relevant="additions text"
            onScroll={event => {
              const element = event.currentTarget;
              followRef.current = element.scrollHeight - element.scrollTop - element.clientHeight < 80;
            }}
            className="min-h-0 flex-1 space-y-3 overflow-y-auto px-4 py-3">
            {state.messages.length === 0 && state.loaded && !state.busy && (
              <p className="mt-4 text-center text-sm text-muted-foreground">Ask about decisions, action items, or something said in this meeting.</p>
            )}
            {state.messages.map(message => (
              <div key={message.id} className={`flex ${message.role === 'user' ? 'justify-end' : 'justify-start'}`}>
                <div className={`max-w-[90%] rounded-lg px-3 py-2 text-sm ${message.role === 'user' ? 'bg-primary text-primary-foreground' : 'bg-muted text-foreground'}`}>
                  {message.role === 'assistant' ? (
                    <div className="prose prose-sm dark:prose-invert max-w-none break-words [&_*]:my-1">
                      <ReactMarkdown remarkPlugins={[remarkGfm]}>{message.content}</ReactMarkdown>
                    </div>
                  ) : <span className="whitespace-pre-wrap break-words">{message.content}</span>}
                  {message.status === 'failed' && <span className="mt-1 block text-xs">Answer failed — question kept for retry</span>}
                </div>
              </div>
            ))}
            {state.busy && <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground">
              <Loader2 aria-hidden="true" className="h-4 w-4 animate-spin motion-reduce:animate-none" />
              {state.busy === 'loading' ? 'Loading chat history…' : state.busy === 'clearing' ? 'Clearing history…' : 'Preparing an answer…'}
            </p>}
          </div>
          {state.error && <div role="alert" className="shrink-0 border-t border-destructive/30 bg-destructive/10 px-4 py-2 text-sm text-destructive">
            {state.error}
            {!state.loaded && !state.busy && <button type="button" onClick={() => void session.load()} className="ml-2 rounded underline focus-visible:ring-2 focus-visible:ring-ring">Retry loading</button>}
          </div>}
          <div className="shrink-0 border-t border-border p-3">
            {!provider || !model ? (
              <p className="text-sm text-muted-foreground">Choose a summary model in the Summary panel to start chatting.</p>
            ) : (
              <form onSubmit={event => { event.preventDefault(); followRef.current = true; void session.send(provider, model); }} className="flex items-end gap-2">
                <label htmlFor={inputId} className="sr-only">Question about this meeting</label>
                <textarea ref={inputRef} id={inputId} value={state.draft}
                  onChange={event => session.setDraft(event.target.value)}
                  onKeyDown={event => {
                    if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
                      event.preventDefault();
                      if (canSend) { followRef.current = true; void session.send(provider, model); }
                    }
                  }}
                  disabled={!state.loaded || Boolean(state.busy)} rows={2} maxLength={8000}
                  placeholder="Ask about this meeting…"
                  className="max-h-28 min-w-0 flex-1 resize-none rounded-lg border border-border bg-background px-3 py-2 text-sm text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50" />
                <button type="submit" disabled={!canSend} aria-label="Send question"
                  className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-primary text-primary-foreground hover:bg-primary/90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-40">
                  <Send aria-hidden="true" className="h-4 w-4" />
                </button>
              </form>
            )}
          </div>
        </section>
      )}
      <button ref={launcherRef} type="button" onClick={() => open ? close() : setOpen(true)}
        aria-label={open ? 'Close meeting chat' : 'Open meeting chat'} aria-expanded={open} aria-controls={panelId}
        className="flex h-12 w-12 items-center justify-center rounded-full bg-primary text-primary-foreground shadow-lg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
        {open ? <X aria-hidden="true" className="h-5 w-5" /> : <MessageSquare aria-hidden="true" className="h-5 w-5" />}
      </button>
      <ConfirmationModal isOpen={confirmClear} title="Clear chat history?" confirmLabel="Clear history"
        text="This removes the chat conversation, not the recording, transcript, or meeting notes. This cannot be undone."
        onCancel={() => setConfirmClear(false)} onConfirm={async () => { await session.clear(); setConfirmClear(false); }} />
    </div>
  );
}
