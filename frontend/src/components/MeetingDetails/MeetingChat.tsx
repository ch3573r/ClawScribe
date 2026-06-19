import React, { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { motion, AnimatePresence } from 'framer-motion';
import { MessageSquare, X, Send, Loader2, Trash2 } from 'lucide-react';

interface AiChatMessage {
  id: string;
  meeting_id: string;
  role: 'user' | 'assistant';
  content: string;
  created_at: string;
}

interface MeetingChatProps {
  meetingId: string;
  /** The configured summary provider + model; chat reuses them. */
  provider?: string;
  model?: string;
}

/**
 * Transcript-grounded "chat with your meeting". A floating widget so it never
 * disturbs the transcript/summary panels: a launcher button bottom-right that
 * expands into a conversation grounded in this meeting's transcript. History is
 * persisted server-side per meeting.
 */
export function MeetingChat({ meetingId, provider, model }: MeetingChatProps) {
  const [open, setOpen] = useState(false);
  const [messages, setMessages] = useState<AiChatMessage[]>([]);
  const [input, setInput] = useState('');
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loadedFor, setLoadedFor] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);

  const canSend = Boolean(provider && model) && !sending && input.trim().length > 0;

  // Load history the first time the panel opens for this meeting.
  useEffect(() => {
    if (!open || loadedFor === meetingId) return;
    (async () => {
      try {
        const history = await invoke<AiChatMessage[]>('api_chat_history', { meetingId });
        setMessages(history);
        setLoadedFor(meetingId);
      } catch (e) {
        setError(e instanceof Error ? e.message : 'Failed to load chat history');
      }
    })();
  }, [open, meetingId, loadedFor]);

  // Keep the newest message in view.
  useEffect(() => {
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight, behavior: 'smooth' });
  }, [messages, sending]);

  const send = async () => {
    const question = input.trim();
    if (!question || sending) return;
    if (!provider || !model) {
      setError('Choose a summary model first (Summary panel → model settings).');
      return;
    }
    setError(null);
    setSending(true);
    // Optimistic user bubble (the backend also persists it).
    const optimistic: AiChatMessage = {
      id: `local-${Date.now()}`,
      meeting_id: meetingId,
      role: 'user',
      content: question,
      created_at: new Date().toISOString(),
    };
    setMessages((m) => [...m, optimistic]);
    setInput('');
    try {
      const reply = await invoke<AiChatMessage>('api_chat_send', {
        meetingId,
        model: provider,
        modelName: model,
        question,
      });
      setMessages((m) => [...m, reply]);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'The model could not answer. Try again.');
    } finally {
      setSending(false);
    }
  };

  const clear = async () => {
    try {
      await invoke('api_chat_clear', { meetingId });
      setMessages([]);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to clear chat');
    }
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  };

  return (
    <div className="fixed bottom-6 right-6 z-50">
      <AnimatePresence>
        {open && (
          <motion.div
            initial={{ opacity: 0, y: 16, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 16, scale: 0.98 }}
            transition={{ duration: 0.18, ease: 'easeOut' }}
            className="mb-3 flex h-[32rem] w-[26rem] max-w-[calc(100vw-3rem)] flex-col overflow-hidden rounded-xl border border-border bg-card shadow-2xl"
          >
            <div className="flex items-center justify-between border-b border-border px-4 py-3">
              <div className="flex items-center gap-2">
                <MessageSquare className="h-4 w-4 text-primary" />
                <h3 className="text-sm font-semibold text-foreground">Chat with this meeting</h3>
              </div>
              <div className="flex items-center gap-1">
                {messages.length > 0 && (
                  <button
                    onClick={clear}
                    title="Clear chat"
                    className="rounded p-1.5 text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive"
                  >
                    <Trash2 className="h-4 w-4" />
                  </button>
                )}
                <button
                  onClick={() => setOpen(false)}
                  title="Close"
                  className="rounded p-1.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                >
                  <X className="h-4 w-4" />
                </button>
              </div>
            </div>

            <div ref={scrollRef} className="flex-1 space-y-3 overflow-y-auto px-4 py-3">
              {messages.length === 0 && !sending && (
                <p className="mt-8 text-center text-sm text-muted-foreground">
                  Ask anything about this meeting — decisions, action items, what someone said.
                  Answers come only from the transcript.
                </p>
              )}
              {messages.map((m) => (
                <div
                  key={m.id}
                  className={`flex ${m.role === 'user' ? 'justify-end' : 'justify-start'}`}
                >
                  <div
                    className={`max-w-[85%] rounded-lg px-3 py-2 text-sm ${
                      m.role === 'user'
                        ? 'bg-primary text-primary-foreground'
                        : 'bg-muted text-foreground'
                    }`}
                  >
                    {m.role === 'assistant' ? (
                      <div className="prose prose-sm dark:prose-invert max-w-none break-words [&_*]:my-1">
                        <ReactMarkdown remarkPlugins={[remarkGfm]}>{m.content}</ReactMarkdown>
                      </div>
                    ) : (
                      <span className="whitespace-pre-wrap break-words">{m.content}</span>
                    )}
                  </div>
                </div>
              ))}
              {sending && (
                <div className="flex justify-start">
                  <div className="flex items-center gap-2 rounded-lg bg-muted px-3 py-2 text-sm text-muted-foreground">
                    <Loader2 className="h-4 w-4 animate-spin" />
                    Thinking…
                  </div>
                </div>
              )}
            </div>

            {error && (
              <div className="border-t border-destructive/30 bg-destructive/10 px-4 py-2 text-xs text-destructive">
                {error}
              </div>
            )}

            <div className="border-t border-border p-3">
              {!provider || !model ? (
                <p className="px-1 text-xs text-muted-foreground">
                  Choose a summary model to start chatting (Summary panel → model settings).
                </p>
              ) : (
                <div className="flex items-end gap-2">
                  <textarea
                    value={input}
                    onChange={(e) => setInput(e.target.value)}
                    onKeyDown={onKeyDown}
                    rows={1}
                    placeholder="Ask about this meeting…"
                    className="max-h-28 flex-1 resize-none rounded-lg border border-border bg-background px-3 py-2 text-sm text-foreground placeholder:text-muted-foreground focus:border-primary focus:outline-none"
                  />
                  <button
                    onClick={send}
                    disabled={!canSend}
                    title="Send"
                    className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-primary text-primary-foreground transition-colors hover:bg-primary/90 disabled:cursor-not-allowed disabled:opacity-40"
                  >
                    <Send className="h-4 w-4" />
                  </button>
                </div>
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>

      <button
        onClick={() => setOpen((o) => !o)}
        title="Chat with this meeting"
        className="flex h-12 w-12 items-center justify-center rounded-full bg-primary text-primary-foreground shadow-lg transition-transform hover:scale-105 active:scale-95"
      >
        {open ? <X className="h-5 w-5" /> : <MessageSquare className="h-5 w-5" />}
      </button>
    </div>
  );
}
