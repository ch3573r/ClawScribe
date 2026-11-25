"use client";

import { useEffect, useState } from "react";
import { usePathname } from "next/navigation";
import { CalendarClock } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Meta } from "@/components/ui/settings-layout";
import { UpcomingSeriesPreparation } from "@/components/MeetingDetails/SeriesPreparation";
import { renderJoinRecordingStatus } from "@/components/UpcomingMeetings";
import { supportsJoinRecording, useJoinRecording } from "@/hooks/useJoinRecording";
import { useUpcomingEvents } from "@/hooks/useUpcomingEvents";
import type { MeetingCalendarLink } from "@/lib/meetingCalendar";
import { upcomingSeriesLink } from "@/lib/meeting-follow-through";
import { describeNextUp, eventDuration, pickNextUp } from "@/lib/sidebarMeetings";

/**
 * The next calendar event in the sidebar. Rendered only while the sidebar is
 * expanded, so calendar requests do not run for the collapsed rail. It renders
 * nothing when there is no upcoming event, brief or active join request.
 */
export function NextUpCard() {
  const { entries, isSelected, select } = useUpcomingEvents(5, true);
  const pathname = usePathname();
  const recording = useJoinRecording();
  const [brief, setBrief] = useState<MeetingCalendarLink | null>(null);
  const [now, setNow] = useState(() => new Date());

  // Keep "in 2 h 18 min" current.
  useEffect(() => {
    const timer = window.setInterval(() => setNow(new Date()), 60_000);
    return () => window.clearInterval(timer);
  }, []);

  const next = pickNextUp(entries, now, 7);
  const timing = next ? describeNextUp(next.event.start!, next.event.end, now) : null;
  const series = next ? upcomingSeriesLink(next) : null;
  const showPending = pathname !== "/" && Boolean(recording.pending);
  if ((!next || !timing) && !showPending && !brief) return null;

  const selected = next ? isSelected(next) : false;
  const duration = next ? eventDuration(next.event.start!, next.event.end) : null;
  return (
    <section aria-label="Next up" className="mt-5 px-3">
      <div className="mb-2 flex items-center justify-between gap-2 px-1 text-xs text-muted-foreground">
        <span>Next up</span>
        <span className="min-w-0 truncate">{next?.label}</span>
      </div>
      <div className="grid gap-1.5 rounded-lg border border-sidebar-border bg-background/60 p-3">
        {next && timing && <>
          <Meta className="flex items-center gap-1.5 text-primary">
            <CalendarClock aria-hidden="true" className="h-3.5 w-3.5 shrink-0" />
            {timing.when}{timing.relative ? ` · ${timing.relative}` : ""}
          </Meta>
          <div className="truncate text-sm font-semibold text-sidebar-foreground" title={next.event.subject || undefined}>
            {next.event.subject || "(No subject)"}
          </div>
          <div className="text-xs text-muted-foreground">
            {duration ? `${duration} · ` : ""}Review attendance during or after recording
          </div>
          <Button type="button" size="sm" variant="outline" className="mt-1.5 w-full" disabled={selected} onClick={() => select(next)}>
            {selected ? "Selected for next recording" : "Use for next recording"}
          </Button>
          {series && <Button type="button" size="sm" variant="ghost" onClick={() => setBrief(series)}>Preparation brief</Button>}
          {recording.windows && supportsJoinRecording(next) && (
            <Button type="button" size="sm" variant="outline" disabled={recording.busy || Boolean(recording.pending)}
              onClick={() => void recording.join(next)}>Join and record</Button>
          )}
        </>}
        {showPending && renderJoinRecordingStatus(recording)}
      </div>
      {brief && <UpcomingSeriesPreparation event={brief} close={() => setBrief(null)} />}
    </section>
  );
}
