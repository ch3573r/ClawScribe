"use client";

import { useEffect, useMemo, useState } from "react";
import { CalendarClock, MoreHorizontal } from "lucide-react";
import { OutlookCalendarIcon } from "@/components/IntegrationIcons";
import { AddCalendarDialog, UseEventButton, formatWhen, type UpcomingEvent } from "@/components/ICalendarSettings";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { SettingsCard, SettingsColumns, SettingsFooter, SettingsRow, StatusText } from "@/components/ui/settings-layout";
import { IconAction, IconTile, NoteRow } from "@/components/settings/settings-parts";
import { useICalendar } from "@/hooks/useICalendar";
import { useMicrosoftExport } from "@/hooks/useMicrosoftExport";
import { usePendingCalendar } from "@/hooks/usePendingCalendar";
import { removeICalendarSource } from "@/services/icalService";
import { microsoftExportService, type CalendarEvent } from "@/services/microsoftExportService";

const CALENDAR_REFRESH_MS = 5 * 60 * 1000;
const PAGE_SIZE = 8;

function sortKey(event: UpcomingEvent) {
  const time = event.start ? Date.parse(event.start) : NaN;
  return Number.isNaN(time) ? Number.MAX_SAFE_INTEGER : time;
}

export function CalendarsSettings() {
  const ms = useMicrosoftExport();
  const { sources, events, issues, loading, error, refresh } = useICalendar();
  const pending = usePendingCalendar();
  const [removing, setRemoving] = useState<string | null>(null);
  const [actionError, setActionError] = useState("");
  const [shown, setShown] = useState(PAGE_SIZE);
  // The shared hook only loads the next day; this list promises a week.
  const [weekEvents, setWeekEvents] = useState<CalendarEvent[]>([]);
  const connected = ms.connection.state === "connected";
  const calendarAvailable = connected && !ms.connection.unavailableExports?.includes("Calendar lookup");
  const accountName = ms.connection.userDisplayName ?? ms.connection.userEmail;

  // Microsoft calendar: load now, then every five minutes and when the window regains focus.
  useEffect(() => {
    if (!calendarAvailable) return;
    let cancelled = false;
    const reload = () => {
      if (cancelled) return;
      void ms.loadCalendar();
      const now = new Date();
      const end = new Date(now);
      end.setDate(now.getDate() + 7);
      microsoftExportService.listCalendarEvents(now.toISOString(), end.toISOString())
        .then(list => { if (!cancelled) setWeekEvents(list); })
        .catch(() => { /* the connection row already reports calendar problems */ });
    };
    reload();
    const interval = window.setInterval(reload, CALENDAR_REFRESH_MS);
    const onVisibility = () => {
      if (document.visibilityState === "visible") reload();
    };
    window.addEventListener("focus", reload);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
      window.removeEventListener("focus", reload);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [calendarAvailable, ms.loadCalendar]);

  const upcoming = useMemo(() => {
    const names = new Map(sources.map(source => [source.id, source.name]));
    const items: UpcomingEvent[] = [];
    if (calendarAvailable) {
      const microsoft = weekEvents.length ? [...weekEvents] : [...ms.calendarEvents];
      if (ms.currentMeeting && !microsoft.some(event => event.id === ms.currentMeeting?.id)) microsoft.unshift(ms.currentMeeting);
      for (const event of microsoft) {
        items.push({
          key: `microsoft:${event.id}`, source: "microsoft", sourceName: "Microsoft 365", id: event.id,
          subject: event.subject, start: event.start, allDay: false,
          attendees: event.attendees.map(a => ({ name: a.name, email: a.email })),
        });
      }
    }
    for (const event of events) {
      items.push({
        key: `ical:${event.sourceId}:${event.id}`, source: "ical", sourceId: event.sourceId,
        sourceName: names.get(event.sourceId) ?? "Calendar", id: event.id, subject: event.subject,
        start: event.start, allDay: event.allDay, attendees: event.attendees.map(a => ({ name: a.name, email: a.email })),
      });
    }
    return items.sort((a, b) => sortKey(a) - sortKey(b) || a.key.localeCompare(b.key));
  }, [calendarAvailable, events, ms.calendarEvents, ms.currentMeeting, sources, weekEvents]);

  async function remove(id: string) {
    setRemoving(id);
    setActionError("");
    try {
      await removeICalendarSource(id);
    } catch (cause) {
      setActionError(cause instanceof Error ? cause.message : "Could not remove this calendar.");
    } finally {
      setRemoving(null);
    }
  }

  function reloadAll() {
    setActionError("");
    if (calendarAvailable) void ms.loadCalendar();
    void refresh().catch(() => setActionError("Could not refresh calendars. Try again."));
  }

  const busy = loading || ms.loadingCalendar;
  const nothing = !connected && sources.length === 0;
  const visible = upcoming.slice(0, shown);

  const sourcesCard = (
    <SettingsCard
      title="Sources"
      actions={
        <div className="flex items-center gap-2">
          <IconAction label="Refresh calendars" onClick={reloadAll} loading={busy} disabled={Boolean(removing)} />
          <AddCalendarDialog />
        </div>
      }
    >
      {connected && (
        <SettingsRow
          icon={<IconTile><OutlookCalendarIcon className="h-5 w-5" /></IconTile>}
          title="Microsoft 365 calendar"
          description={calendarAvailable
            ? [accountName, "includes invitees"].filter(Boolean).join(" · ")
            : "Calendar permission is unavailable. Request it under Microsoft 365."}
        >
          <StatusText tone={calendarAvailable ? "ok" : "warning"}>{calendarAvailable ? "Connected" : "Needs permission"}</StatusText>
        </SettingsRow>
      )}
      {sources.map(source => {
        const count = events.filter(event => event.sourceId === source.id).length;
        const issue = issues[source.id];
        return (
          <SettingsRow
            key={source.id}
            icon={<IconTile><CalendarClock aria-hidden="true" /></IconTile>}
            title={<span className="break-words">{source.name}</span>}
            description={<>
              {source.kind === "subscription" ? "Subscription" : "Imported file"} · {count} upcoming
              {issue && <span className="block text-error-foreground">{issue}</span>}
            </>}
          >
            {issue && <StatusText tone="warning">Needs attention</StatusText>}
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button aria-label={`Manage ${source.name}`} variant="ghost" size="icon" disabled={Boolean(removing)}>
                  <MoreHorizontal aria-hidden="true" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem onSelect={() => void remove(source.id)}>Remove calendar</DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </SettingsRow>
        );
      })}
      {nothing && !busy && (
        <SettingsRow title="No calendars yet" description="Sign in to Microsoft 365, or add an .ics file or a subscription" />
      )}
      {(error || actionError) && <NoteRow tone="error">{actionError || error}</NoteRow>}
      <SettingsFooter>
        {removing ? <span role="status">Removing calendar…</span>
          : busy ? <span role="status">Loading calendars…</span>
          : <span>Files and subscriptions up to 16 MiB. Descriptions and attachments are not kept.</span>}
      </SettingsFooter>
    </SettingsCard>
  );

  const upcomingCard = (
    <SettingsCard title="Next 7 days">
      {visible.map(event => (
        <SettingsRow
          key={event.key}
          title={<span className="break-words">{event.subject || "Untitled meeting"}</span>}
          description={<span className="break-words font-mono text-xs tabular-nums">{formatWhen(event.start, event.allDay)} · {event.sourceName}</span>}
          className="min-h-14"
        >
          <UseEventButton event={event} pending={pending} />
        </SettingsRow>
      ))}
      {!visible.length && (
        <SettingsRow
          title={busy ? "Loading…" : nothing ? "No calendars yet" : "No meetings in the next 7 days"}
          description={busy || !nothing ? undefined : "Add a calendar to see upcoming meetings here"}
        />
      )}
      {upcoming.length > shown && (
        <SettingsFooter actions={<Button type="button" variant="ghost" size="sm" onClick={() => setShown(count => count + PAGE_SIZE)}>Show more</Button>}>
          <span>{shown} of {upcoming.length} meetings</span>
        </SettingsFooter>
      )}
      <SettingsFooter>Also shown on Home, above recent meetings</SettingsFooter>
    </SettingsCard>
  );

  return <SettingsColumns aside={upcomingCard}>{sourcesCard}</SettingsColumns>;
}
