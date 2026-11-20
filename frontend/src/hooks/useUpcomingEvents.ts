"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { useICalendar } from "@/hooks/useICalendar";
import { usePendingCalendar } from "@/hooks/usePendingCalendar";
import { calendarLinkFromEvent, sameCalendarEvent, setPendingCalendar } from "@/lib/meetingCalendar";
import { toast } from "sonner";
import {
  microsoftExportService,
  type CalendarEvent,
} from "@/services/microsoftExportService";

const CALENDAR_REFRESH_MS = 5 * 60 * 1000;

export interface UpcomingEntry {
  event: CalendarEvent;
  /** Set for iCalendar sources, null for Microsoft 365. */
  sourceId: string | null;
  label: string;
  allDay: boolean;
}

/**
 * Upcoming events from Microsoft 365 and imported or subscribed calendars, soonest
 * first, for Home and the sidebar. Calendar requests are skipped while Microsoft
 * 365 is not connected or has no calendar permission.
 */
export function useUpcomingEvents(limit = 5, timedOnly = false) {
  const [microsoftEvents, setMicrosoftEvents] = useState<CalendarEvent[]>([]);
  const [microsoftAvailable, setMicrosoftAvailable] = useState(false);
  const ical = useICalendar();
  const pending = usePendingCalendar();
  const readVersion = useRef(0);

  const refresh = useCallback(async (cancelled: () => boolean) => {
    const version = ++readVersion.current;
    const expired = () => cancelled() || version !== readVersion.current;
    try {
      const connection = await microsoftExportService.connectionStatus();
      if (expired()) return;
      if (connection.state !== "connected" || connection.unavailableExports?.includes("Calendar lookup")) {
        setMicrosoftAvailable(false);
        setMicrosoftEvents([]);
        return;
      }
      setMicrosoftAvailable(true);
      const now = new Date();
      const end = new Date(now);
      end.setDate(now.getDate() + 7);
      const list = await microsoftExportService.listCalendarEvents(
        now.toISOString(),
        end.toISOString(),
      );
      if (!expired()) setMicrosoftEvents(list);
    } catch {
      if (!expired()) setMicrosoftEvents([]);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    const isCancelled = () => cancelled;
    const run = () => void refresh(isCancelled);
    run();
    const interval = window.setInterval(run, CALENDAR_REFRESH_MS);
    const onVisibility = () => {
      if (document.visibilityState === "visible") run();
    };
    window.addEventListener("focus", run);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
      window.removeEventListener("focus", run);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [refresh]);

  const now = Date.now();
  const upcoming = (event: CalendarEvent) => event.start && Date.parse(event.end ?? event.start) >= now;
  // Both sources supply chronological lists, so the first `limit` of each are enough.
  // All-day entries are dropped before the cut when only timed events are wanted,
  // or a run of holidays could hide the next real meeting.
  const entries: UpcomingEntry[] = [
    ...microsoftEvents.filter(event => upcoming(event) && !(timedOnly && event.allDay))
      .slice(0, limit).map(event => ({ event, sourceId: null, label: "Microsoft 365", allDay: Boolean(event.allDay) })),
    ...ical.events.filter(event => upcoming(event) && !(timedOnly && event.allDay)).slice(0, limit).map(event => ({ event, sourceId: event.sourceId, allDay: event.allDay,
      label: ical.sources.find(source => source.id === event.sourceId)?.name ?? "iCalendar" })),
  ].sort((a, b) => Date.parse(a.event.start!) - Date.parse(b.event.start!))
    .slice(0, limit);

  const isSelected = (entry: UpcomingEntry) =>
    sameCalendarEvent(pending, calendarLinkFromEvent({ ...entry.event, sourceId: entry.sourceId }));

  /** Use this event's title and invitees for the next recording. */
  const select = (entry: UpcomingEntry) => {
    const { event, sourceId } = entry;
    const saved = setPendingCalendar(calendarLinkFromEvent({ ...event, sourceId }));
    if (!saved) toast.error("Could not save the calendar selection on this device.");
  };

  return { entries, isSelected, select, hasCalendars: microsoftAvailable || ical.sources.length > 0 };
}
