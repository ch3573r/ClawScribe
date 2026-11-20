"use client";

import { invoke } from "@tauri-apps/api/core";
import {
  LEGACY_MEETING_CALENDAR_PREFIX, parseCalendarLink, type MeetingCalendarLink,
} from "@/lib/meetingCalendar";

let migration: Promise<void> | undefined;
export const MEETING_SERIES_TEMPLATE_CHANGED = "clawscribe-meeting-series-template-changed";

export function migrateLegacyMeetingCalendars(): Promise<void> {
  if (typeof window === "undefined") return Promise.resolve();
  if (!migration) {
    migration = (async () => {
      let store: Storage;
      let keys: (string | null)[];
      try {
        store = window.localStorage;
        keys = Array.from({ length: store.length }, (_, index) => store.key(index));
      } catch { return; }
      for (const key of keys) {
        if (!key?.startsWith(LEGACY_MEETING_CALENDAR_PREFIX)) continue;
        const link = parseCalendarLink(store.getItem(key));
        if (!link) continue;
        await invoke("set_meeting_calendar", {
          meetingId: key.slice(LEGACY_MEETING_CALENDAR_PREFIX.length),
          link,
          onlyIfMissing: true,
        });
        store.removeItem(key);
      }
    })().catch(error => { migration = undefined; throw error; });
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
