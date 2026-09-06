/** A single meeting's chat state. Synchronous gates protect double clicks;
 * generation checks prevent stale async completions from replacing newer state.
 * No React setter side effects and no dependency on an installed Tauri runtime.
 */
export interface ChatMessage {
  id: string;
  meeting_id: string;
  role: 'user' | 'assistant';
  content: string;
  created_at: string;
  status?: 'pending' | 'failed';
  context_truncated?: boolean;
}

export type ChatInvoke = <T>(command: string, args: Record<string, unknown>) => Promise<T>;
export interface ChatSnapshot {
  messages: ChatMessage[];
  draft: string;
  loaded: boolean;
  busy: 'loading' | 'sending' | 'clearing' | null;
  error: string | null;
  contextTruncated: boolean;
}

export class MeetingChatSession {
  private snapshot: ChatSnapshot = {
    messages: [], draft: '', loaded: false, busy: null, error: null, contextTruncated: false,
  };
  private listeners = new Set<() => void>();
  private epoch = 0;
  private retry: { id: string; question: string } | null = null;

  constructor(private readonly meetingId: string, private readonly invoke: ChatInvoke) {}

  getSnapshot = (): ChatSnapshot => this.snapshot;
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
  private update(patch: Partial<ChatSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...patch };
    this.listeners.forEach(listener => listener());
  }
  setDraft = (draft: string): void => { this.update({ draft }); };

  /** Invalidate callbacks on unmount, including a meeting switch. */
  invalidate = (): void => {
    this.epoch++;
    this.update({ loaded: false, busy: null });
  };

  load = async (): Promise<void> => {
    if (this.snapshot.loaded || this.snapshot.busy) return;
    const epoch = ++this.epoch;
    this.update({ busy: 'loading', error: null });
    try {
      const messages = await this.invoke<ChatMessage[]>('api_chat_history', { meetingId: this.meetingId });
      if (epoch !== this.epoch) return;
      // Reject cross-meeting data rather than accidentally displaying it.
      if (messages.some(message => message.meeting_id !== this.meetingId)) throw new Error('Meeting mismatch');
      this.update({ messages, loaded: true, busy: null });
    } catch {
      if (epoch === this.epoch) this.update({ busy: null, error: 'Could not load chat history. Retry before sending a question.' });
    }
  };

  send = async (provider?: string, model?: string): Promise<void> => {
    const question = this.snapshot.draft.trim();
    if (!this.snapshot.loaded || this.snapshot.busy || !question) return;
    if (!provider || !model) {
      this.update({ error: 'Choose a summary model before sending a question.' });
      return;
    }
    const epoch = this.epoch;
    const request = this.retry?.question === question
      ? this.retry
      : { id: `chat-${globalThis.crypto.randomUUID()}`, question };
    this.retry = request;
    const optimistic: ChatMessage = {
      id: request.id, meeting_id: this.meetingId, role: 'user', content: question,
      created_at: new Date().toISOString(), status: 'pending',
    };
    this.update({
      busy: 'sending', draft: '', error: null,
      messages: [...this.snapshot.messages.filter(message => message.id !== request.id), optimistic],
    });
    try {
      const reply = await this.invoke<ChatMessage>('api_chat_send', {
        meetingId: this.meetingId, model: provider, modelName: model,
        question, requestId: request.id,
      });
      if (epoch !== this.epoch) return;
      if (reply.meeting_id !== this.meetingId || reply.role !== 'assistant') throw new Error('Meeting mismatch');
      this.retry = null;
      this.update({
        messages: [
          ...this.snapshot.messages.filter(message => message.id !== reply.id)
            .map(message => message.id === request.id ? { ...message, status: undefined } : message),
          reply,
        ],
        busy: null, contextTruncated: Boolean(reply.context_truncated),
      });
    } catch {
      if (epoch !== this.epoch) return;
      this.update({
        busy: null,
        draft: this.snapshot.draft || question,
        messages: this.snapshot.messages.map(message => message.id === request.id ? { ...message, status: 'failed' } : message),
        error: 'The answer could not be completed. Your question is kept below; send again to retry.',
      });
    }
  };

  clear = async (): Promise<void> => {
    // Clear never races an active send, in the UI or through a stale handler.
    // The native command enforces the same per-meeting exclusion.
    if (!this.snapshot.loaded || this.snapshot.busy) return;
    const epoch = ++this.epoch;
    this.update({ busy: 'clearing', error: null });
    try {
      await this.invoke('api_chat_clear', { meetingId: this.meetingId });
      if (epoch !== this.epoch) return;
      this.retry = null;
      this.update({ messages: [], busy: null, contextTruncated: false });
    } catch {
      if (epoch === this.epoch) this.update({ busy: null, error: 'Could not clear chat. The conversation has been kept; please retry.' });
    }
  };
}
