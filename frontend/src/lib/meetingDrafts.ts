import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';

export type SummaryDraft = { markdown?: string; summary_json?: unknown[]; [key: string]: unknown };
type Edits = { title?: string; summary?: SummaryDraft; readSummary?: () => Promise<SummaryDraft> };
export type SaveState = 'saved' | 'unsaved' | 'saving' | 'error';

// Capture each revision's data before awaiting disk writes. This queue outlives
// its page so navigation and failed saves cannot discard the pending revision.
export function createDraftQueue(write: (edits: Edits) => Promise<void>, changed: () => void = () => {}) {
  let edits: Edits = {}, revision = 0, state: SaveState = 'saved';
  let timer: ReturnType<typeof setTimeout> | undefined;
  let inFlight: Promise<void> | undefined;
  const listeners = new Set<() => void>();
  const notify = () => { changed(); listeners.forEach(listener => listener()); };
  const dirty = () => edits.title !== undefined || edits.summary !== undefined;
  const schedule = () => {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => { void flush().catch(reportSaveError); }, 2000);
  };
  const edit = (patch: Edits) => {
    edits = { ...edits, ...patch };
    revision++;
    state = 'unsaved';
    notify();
    schedule();
  };
  const flush = async (): Promise<void> => {
    if (timer) clearTimeout(timer);
    if (inFlight) { await inFlight; if (dirty()) await flush(); return; }
    if (!dirty()) return;
    const snapshot = edits, savedRevision = revision;
    state = 'saving'; notify();
    inFlight = (async () => {
      try {
        await write(snapshot);
        if (savedRevision === revision) { edits = {}; state = 'saved'; }
        else state = 'unsaved';
      } catch (error) {
        state = 'error';
        throw error;
      } finally { notify(); }
    })();
    try { await inFlight; } finally { inFlight = undefined; }
    if (dirty()) schedule();
  };
  const drain = async () => { while (dirty()) await flush(); };
  return {
    edit, flush, drain, dirty,
    get edits() { return edits; },
    get state() { return state; },
    subscribe(listener: () => void) { listeners.add(listener); return () => { listeners.delete(listener); }; },
    detach() { if (timer) clearTimeout(timer); void drain().catch(reportSaveError); },
  };
}

export function reportSaveError() {
  toast.error('Could not save meeting edits', {
    description: 'Your edits are still pending. Wait for generation to finish, then retry saving.',
    action: { label: 'Retry', onClick: () => { void flushMeetingDrafts().catch(() => {}); } },
  });
}

const drafts = new Map<string, ReturnType<typeof createDraftQueue>>();
const busy = (status?: string) => ['pending', 'processing', 'summarizing', 'regenerating'].includes(status?.toLowerCase() ?? '');
let pendingUpdate: Promise<unknown> = Promise.resolve();
const publishPending = () => {
  pendingUpdate = pendingUpdate.then(() => invoke('api_set_summary_edits_pending', { pending: [...drafts.values()].some(draft => draft.dirty()) })).catch(() => {});
};

export function meetingDraft(meetingId: string) {
  let draft = drafts.get(meetingId);
  if (!draft) {
    draft = createDraftQueue(async edits => {
      const current = await invoke<{ status: string }>('api_get_summary', { meetingId });
      if (busy(current?.status)) throw new Error('Summary generation is still running.');
      if (edits.title !== undefined) await invoke('api_save_meeting_title', { meetingId, title: edits.title });
      if (edits.summary !== undefined) {
        const summary = edits.readSummary ? await edits.readSummary() : edits.summary;
        await invoke('api_save_meeting_summary', { meetingId, summary });
      }
    }, publishPending);
    drafts.set(meetingId, draft);
  }
  return draft;
}

export async function flushMeetingDrafts() {
  try {
    await Promise.all([...drafts.values()].map(draft => draft.drain()));
    publishPending();
    await pendingUpdate;
  } catch (error) { reportSaveError(); throw error; }
}

export async function confirmSummaryRegeneration(
  flush: () => Promise<void>,
  read: () => Promise<{ data?: { user_edited_at?: unknown } }>,
  confirm: (message: string) => Promise<boolean>,
) {
  await flush();
  const current = await read();
  return !current?.data?.user_edited_at || await confirm('Regenerate will replace your edited summary. You can restore it afterwards.');
}
