"use client";

import { openExternal } from '@/lib/openExternal';
import { Button } from "@/components/ui/button";
import { useUpcomingEvents } from "@/hooks/useUpcomingEvents";
import { CalendarClock, Video } from "lucide-react";

function formatWhen(iso: string | null, allDay: boolean): string {
  if (!iso) return "";
  const d = new Date(iso);
  const now = new Date();
  if (allDay) return d.toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" }) + " · All day";
  const tomorrow = new Date(now);
  tomorrow.setDate(now.getDate() + 1);
  const time = d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  if (d.toDateString() === now.toDateString()) return `Today · ${time}`;
  if (d.toDateString() === tomorrow.toDateString()) return `Tomorrow · ${time}`;
  return `${d.toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" })} · ${time}`;
}

export function UpcomingMeetings() {
  const { entries: events, isSelected, select } = useUpcomingEvents(5);
  if (!events.length) return null;

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
        {events.map((entry) => {
          const { event: ev, sourceId, allDay, label } = entry;
          const selected = isSelected(entry);
          return (
            <div key={(sourceId ?? "microsoft") + ":" + ev.id} className="flex flex-wrap items-center justify-between gap-4 px-6 py-4">
              <div className="min-w-0">
                <div className="truncate font-medium text-foreground">{ev.subject || "(No subject)"}</div>
                <div className="mt-1 font-mono text-xs text-muted-foreground">
                  {formatWhen(ev.start, allDay)} · {label}
                </div>
              </div>
              <div className="flex shrink-0 items-center gap-2">
                <Button size="sm" variant="ghost" onClick={() => select(entry)} disabled={selected}>
                  {selected ? "Selected for next recording" : "Use for next recording"}
                </Button>
                {ev.joinUrl && (
                  <a href={ev.joinUrl}
                    onClick={event => { event.preventDefault(); void openExternal(event.currentTarget.href); }}
                    rel="noreferrer"
                    className="inline-flex shrink-0 items-center gap-1.5 rounded-md border border-border bg-muted px-3 py-1.5 text-xs font-medium text-foreground transition hover:border-primary/30 hover:bg-primary/10 hover:text-primary">
                    <Video className="h-3.5 w-3.5" />Join
                  </a>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}
