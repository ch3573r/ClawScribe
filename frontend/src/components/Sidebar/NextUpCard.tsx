"use client";

import { useEffect, useState } from "react";
import { CalendarClock } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Meta } from "@/components/ui/settings-layout";
import { useUpcomingEvents } from "@/hooks/useUpcomingEvents";
import { describeNextUp, eventDuration, pickNextUp } from "@/lib/sidebarMeetings";

/**
 * The next calendar event in the sidebar. Rendered only while the sidebar is
 * expanded, so calendar requests do not run for the collapsed rail. It renders
 * nothing when no connected calendar has an event in the next 7 days.
 */
export function NextUpCard() {
  const { entries, isSelected, select } = useUpcomingEvents(5);
  const [now, setNow] = useState(() => new Date());

  // Keep "in 2 h 18 min" current.
  useEffect(() => {
    const timer = window.setInterval(() => setNow(new Date()), 60_000);
    return () => window.clearInterval(timer);
  }, []);

  const next = pickNextUp(entries, now, 7);
  const timing = next ? describeNextUp(next.event.start!, next.event.end, now) : null;
  if (!next || !timing) return null;

  const selected = isSelected(next);
  const duration = eventDuration(next.event.start!, next.event.end);
  return (
    <section aria-label="Next up" className="mt-5 px-3">
      <div className="mb-2 flex items-center justify-between gap-2 px-1 text-xs text-muted-foreground">
        <span>Next up</span>
        <span className="min-w-0 truncate">{next.label}</span>
      </div>
      <div className="grid gap-1.5 rounded-lg border border-sidebar-border bg-background/60 p-3">
        <Meta className="flex items-center gap-1.5 text-primary">
          <CalendarClock aria-hidden="true" className="h-3.5 w-3.5 shrink-0" />
          {timing.when}{timing.relative ? ` · ${timing.relative}` : ""}
        </Meta>
        <div className="truncate text-sm font-semibold text-sidebar-foreground" title={next.event.subject || undefined}>
          {next.event.subject || "(No subject)"}
        </div>
        <div className="text-xs text-muted-foreground">
          {duration ? `${duration} · ` : ""}Title and invitees are filled in for you
        </div>
        <Button type="button" size="sm" variant="outline" className="mt-1.5 w-full" disabled={selected} onClick={() => select(next)}>
          {selected ? "Selected for next recording" : "Use for next recording"}
        </Button>
      </div>
    </section>
  );
}
