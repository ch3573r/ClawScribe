"use client";

import { useEffect, useState } from "react";
import { CalendarClock, Video } from "lucide-react";
import {
  microsoftExportService,
  type CalendarEvent,
} from "@/services/microsoftExportService";

function formatWhen(iso: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  const now = new Date();
  const tomorrow = new Date(now);
  tomorrow.setDate(now.getDate() + 1);
  const time = d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  if (d.toDateString() === now.toDateString()) return `Today · ${time}`;
  if (d.toDateString() === tomorrow.toDateString()) return `Tomorrow · ${time}`;
  return `${d.toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" })} · ${time}`;
}

/**
 * Upcoming meetings from the connected Microsoft calendar. Renders nothing when
 * the calendar isn't connected or there's nothing in the next week — so it adds
 * signal without taking space when it has none.
 */
export function UpcomingMeetings() {
  const [events, setEvents] = useState<CalendarEvent[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const now = new Date();
        const end = new Date(now);
        end.setDate(now.getDate() + 7);
        const list = await microsoftExportService.listCalendarEvents(
          now.toISOString(),
          end.toISOString(),
        );
        if (!cancelled) setEvents(list.slice(0, 5));
      } catch {
        // Not connected / no calendar permission — just stay hidden.
        if (!cancelled) setEvents([]);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  if (!events || events.length === 0) return null;

  return (
    <section className="rounded-lg border border-border bg-card shadow-sm">
      <div className="flex items-center justify-between border-b border-border px-6 py-5">
        <div className="flex items-center gap-2">
          <CalendarClock className="h-4 w-4 text-primary" />
          <h2 className="text-lg font-semibold text-foreground">Upcoming meetings</h2>
        </div>
        <span className="text-xs text-muted-foreground">Next 7 days · from your calendar</span>
      </div>
      <div className="divide-y divide-border">
        {events.map((ev) => (
          <div key={ev.id} className="flex items-center justify-between gap-4 px-6 py-4">
            <div className="min-w-0">
              <div className="truncate font-medium text-foreground">
                {ev.subject || "(No subject)"}
              </div>
              <div className="mt-1 font-mono text-xs text-muted-foreground">
                {formatWhen(ev.start)}
              </div>
            </div>
            {ev.joinUrl && (
              <a
                href={ev.joinUrl}
                target="_blank"
                rel="noreferrer"
                className="inline-flex shrink-0 items-center gap-1.5 rounded-md border border-border bg-muted px-3 py-1.5 text-xs font-medium text-foreground transition hover:border-primary/30 hover:bg-primary/10 hover:text-primary"
              >
                <Video className="h-3.5 w-3.5" />
                Join
              </a>
            )}
          </div>
        ))}
      </div>
    </section>
  );
}
