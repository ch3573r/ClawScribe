"use client";
import { invoke } from '@tauri-apps/api/core';

const PREFIX = 'clawscribe.meetingContext.';
let migration: Promise<void> | undefined;
const writes = new Map<string, Promise<void>>();

export function migrateLegacyMeetingContexts(): Promise<void> {
  if (typeof window === 'undefined') return Promise.resolve();
  if (!migration) {
    migration = (async () => {
      let storage: Storage;
      let keys: string[];
      try {
        storage = window.localStorage;
        keys = Array.from({ length: storage.length }, (_, index) => storage.key(index))
          .filter((key): key is string => Boolean(key?.startsWith(PREFIX)));
      } catch { return; } // SQLite stays usable if browser storage is unavailable.
      for (const key of keys) {
        const context = storage.getItem(key);
        if (context === null) continue;
        await invoke('set_meeting_context', { meetingId: key.slice(PREFIX.length), context, onlyIfMissing: true });
        // A failed database save leaves the recovery value available for retry.
        storage.removeItem(key);
      }
    })().catch(error => { migration = undefined; throw error; });
  }
  return migration;
}

export async function getMeetingContext(meetingId: string): Promise<string> {
  if (!meetingId) return '';
  await migrateLegacyMeetingContexts();
  await writes.get(meetingId);
  return invoke<string>('get_meeting_context', { meetingId });
}

export function setMeetingContext(meetingId: string, context: string): Promise<void> {
  if (!meetingId) return Promise.resolve();
  const previous = writes.get(meetingId) ?? Promise.resolve();
  const next = previous.catch(() => {}).then(async () => {
    await migrateLegacyMeetingContexts();
    await invoke('set_meeting_context', { meetingId, context, onlyIfMissing: false });
  });
  writes.set(meetingId, next);
  void next.then(() => { if (writes.get(meetingId) === next) writes.delete(meetingId); }, () => {});
  return next;
}

export async function flushMeetingContexts(): Promise<void> {
  await migrateLegacyMeetingContexts();
  await Promise.all(writes.values());
}
