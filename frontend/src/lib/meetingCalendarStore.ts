"use client";

import { invoke } from "@tauri-apps/api/core";
import {
  LEGACY_MEETING_CALENDAR_PREFIX, parseCalendarLink, type MeetingCalendarLink,
} from "@/lib/meetingCalendar";

export const MEETING_SERIES_TEMPLATE_CHANGED = "clawscribe-meeting-series-template-changed";
/** Entries the core can never save are moved here so they stop being retried. */
export const REJECTED_MEETING_CALENDAR_PREFIX = "clawscribe.meetingCalendarRejected.";
const TRANSIENT_RETRY_MS = 5_000;

type LegacyOutcome = "migrated" | "invalid" | "retry" | "failed";

let migration: Promise<void> | undefined;
let retryNotBefore = 0;

function setAside(store: Storage, key: string, raw: string | null) {
  try {
    if (raw !== null) store.setItem(REJECTED_MEETING_CALENDAR_PREFIX + key.slice(LEGACY_MEETING_CALENDAR_PREFIX.length), raw);
    store.removeItem(key);
  } catch { /* Leave it in place; it is isolated either way. */ }
}

/** Resolves true when nothing is left to retry. Each entry succeeds or fails alone. */
async function migrateEntries(): Promise<boolean> {
  let store: Storage;
  let keys: (string | null)[];
  try {
    store = window.localStorage;
    keys = Array.from({ length: store.length }, (_, index) => store.key(index));
  } catch { return true; }
  const entries: { key: string; meetingId: string; link: MeetingCalendarLink }[] = [];
  for (const key of keys) {
    if (!key?.startsWith(LEGACY_MEETING_CALENDAR_PREFIX)) continue;
    const raw = store.getItem(key);
    const link = parseCalendarLink(raw);
    if (link) entries.push({ key, meetingId: key.slice(LEGACY_MEETING_CALENDAR_PREFIX.length), link });
    else setAside(store, key, raw);
  }
  if (entries.length === 0) return true;
  let results: { meetingId: string; outcome: LegacyOutcome }[];
  try {
    results = await invoke("migrate_legacy_meeting_calendars", {
      entries: entries.map(({ meetingId, link }) => ({ meetingId, link })),
    });
  } catch { return false; }
  const outcomes = new Map(results.map(result => [result.meetingId, result.outcome]));
  let complete = true;
  for (const { key, meetingId } of entries) {
    const outcome = outcomes.get(meetingId);
    if (outcome === "migrated") store.removeItem(key);
    else if (outcome === "invalid") setAside(store, key, store.getItem(key));
    // Only a transient failure is retried; a lasting one keeps the old copy untouched.
    else if (outcome !== "failed") complete = false;
  }
  return complete;
}

/**
 * Moves old webview-stored links into the library. It never rejects, so a bad or
 * unsavable entry cannot block attendance reads, backup or restore.
 */
export function migrateLegacyMeetingCalendars(): Promise<void> {
  if (typeof window === "undefined" || (!migration && Date.now() < retryNotBefore)) return Promise.resolve();
  if (!migration) {
    migration = migrateEntries().then(
      complete => { if (!complete) { retryNotBefore = Date.now() + TRANSIENT_RETRY_MS; migration = undefined; } },
      () => { retryNotBefore = Date.now() + TRANSIENT_RETRY_MS; migration = undefined; },
    );
  }
  return migration;
}

export async function getMeetingCalendar(meetingId: string): Promise<MeetingCalendarLink | null> {
  await migrateLegacyMeetingCalendars();
  return invoke<MeetingCalendarLink | null>("get_meeting_calendar", { meetingId });
}

export async function setMeetingCalendar(meetingId: string, link: MeetingCalendarLink | null) {
  await migrateLegacyMeetingCalendars();
  await invoke("set_meeting_calendar", { meetingId, link });
}
