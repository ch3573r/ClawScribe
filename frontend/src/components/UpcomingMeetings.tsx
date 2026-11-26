"use client";

import { openExternal } from '@/lib/openExternal';
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { useUpcomingEvents } from "@/hooks/useUpcomingEvents";
import { supportsJoinRecording, useJoinRecording } from "@/hooks/useJoinRecording";
import { CalendarClock, Video } from "lucide-react";
import { toast } from "sonner";
import { setPendingCalendar, type MeetingCalendarLink } from "@/lib/meetingCalendar";
import { UpcomingSeriesPreparation } from "@/components/MeetingDetails/SeriesPreparation";
import { upcomingSeriesLink } from "@/lib/meeting-follow-through";

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

function NextRecordingCalendar({ hasCalendars, choice }: {
  hasCalendars: boolean;
  choice: ReturnType<typeof useUpcomingEvents>["choice"];
}) {
  if (!hasCalendars && !choice.link && !choice.skip) return null;
  const label = choice.link ? `Next recording · ${choice.link.subject || "Untitled event"}`
    : choice.skip ? "Next recording will start without a calendar event" : "";
  return (
    <div className="flex flex-wrap items-center justify-between gap-2 px-6 py-2">
      {label && <span role="status" className="min-w-0 break-words text-sm text-muted-foreground">{label}</span>}
      {choice.link && <Button size="sm" variant="ghost" onClick={() => {
        if (!setPendingCalendar(null)) toast.error("Could not clear the calendar selection.");
      }}>Clear selection</Button>}
      <Button size="sm" variant="ghost" onClick={() => {
        if (!setPendingCalendar(null, !choice.skip)) toast.error("Could not change the calendar selection.");
      }}>
        {choice.skip ? "Allow calendar matching" : "Skip calendar for next recording"}
      </Button>
    </div>
  );
}

export function renderJoinRecordingStatus(recording: ReturnType<typeof useJoinRecording>) {
  const { pending } = recording;
  if (!pending) return null;
  return <>
    <span role="status" className="min-w-0 flex-1 break-words text-sm text-muted-foreground">
      {pending.opening ? `Opening “${pending.subject}”…`
        : pending.promptKey ? `Is this the call for “${pending.subject}”?`
        : `Waiting for the call for “${pending.subject}”.`}
    </span>
    {pending.promptKey && <Button size="sm" disabled={recording.busy} onClick={recording.confirm}>Record this call</Button>}
    <Button size="sm" variant="ghost" onClick={recording.cancel}>Cancel recording request</Button>
  </>;
}

export function UpcomingMeetings() {
  const { entries: events, choice, isSelected, select, hasCalendars } = useUpcomingEvents(5);
  const recording = useJoinRecording();
  const [brief, setBrief] = useState<MeetingCalendarLink | null>(null);
  if (!events.length && !hasCalendars && !recording.pending && !choice.link && !choice.skip) return null;

  return (
    <section className="rounded-lg border border-border bg-card shadow-sm">
      <div className="flex items-center justify-between border-b border-border px-6 py-5">
        <div className="flex items-center gap-2">
          <CalendarClock className="h-4 w-4 text-primary" />
          <h2 className="text-lg font-semibold text-foreground">Upcoming meetings</h2>
        </div>
        <span className="text-xs text-muted-foreground">Next 7 days</span>
      </div>
      <div className="divide-y divide-border">
        {events.map((entry) => {
          const { event: ev, sourceId, allDay, label } = entry;
          const selected = isSelected(entry);
          const series = upcomingSeriesLink(entry);
          return (
            <div key={(sourceId ?? "microsoft") + ":" + ev.id} className="flex flex-wrap items-center justify-between gap-4 px-6 py-4">
              <div className="min-w-0">
                <div className="truncate font-medium text-foreground">{ev.subject || "(No subject)"}</div>
                <div className="mt-1 font-mono text-xs text-muted-foreground">
                  {formatWhen(ev.start, allDay)} · {label}
                </div>
              </div>
              <div className="flex flex-wrap items-center gap-2">
                {series && <Button size="sm" variant="ghost" onClick={() => setBrief(series)}>Preparation brief</Button>}
                <Button size="sm" variant="ghost" onClick={() => select(entry)} disabled={selected}>
                  {selected ? "Selected for next recording" : "Use for next recording"}
                </Button>
                {ev.joinUrl && (
                  <Button size="sm" variant="outline" onClick={() => void openExternal(ev.joinUrl!)}>
                    <Video className="h-3.5 w-3.5" />Join
                  </Button>
                )}
                {recording.windows && supportsJoinRecording(entry) && (
                  <Button size="sm" variant="outline" disabled={recording.busy || Boolean(recording.pending)}
                    onClick={() => void recording.join(entry)}>Join and record</Button>
                )}
              </div>
            </div>
          );
        })}
      </div>
      {recording.pending && (
        <div className="flex flex-wrap items-center gap-2 border-t border-border px-6 py-2">
          {renderJoinRecordingStatus(recording)}
        </div>
      )}
      <NextRecordingCalendar hasCalendars={hasCalendars} choice={choice} />
      {brief && <UpcomingSeriesPreparation event={brief} close={() => setBrief(null)} />}
    </section>
  );
}
