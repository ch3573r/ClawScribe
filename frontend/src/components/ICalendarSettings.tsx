"use client";

import { useRef, useState } from "react";
import { MoreHorizontal, RefreshCw } from "lucide-react";
import { useICalendar } from "@/hooks/useICalendar";
import { usePendingCalendar } from "@/hooks/usePendingCalendar";
import { setPendingCalendar, type MeetingCalendarLink } from "@/lib/meetingCalendar";
import {
  addICalendarSubscription, importICalendarFile, removeICalendarSource, type ICalendarEvent,
} from "@/services/icalService";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle, DialogTrigger,
} from "@/components/ui/dialog";
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { PageSection } from "@/components/ui/page-section";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

function AddCalendarDialog() {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const saving = useRef(false);

  function changeOpen(next: boolean) {
    if (saving.current) return;
    setName("");
    setUrl("");
    setError("");
    setOpen(next);
  }

  async function add(action: () => Promise<void>, fallback: string) {
    if (saving.current) return;
    saving.current = true;
    setBusy(true);
    setError("");
    try {
      await action();
      setUrl("");
      setName("");
      setOpen(false);
    } catch (cause) {
      // Calendar service mutations expose only user-safe messages.
      setError(cause instanceof Error ? cause.message : fallback);
    } finally {
      saving.current = false;
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={changeOpen}>
      <DialogTrigger asChild><Button variant="outline" size="sm">Add calendar</Button></DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Add calendar</DialogTitle>
          <DialogDescription>Import a local file or subscribe to updates from a read-only calendar.</DialogDescription>
        </DialogHeader>
        <Tabs defaultValue="file" onValueChange={() => setError("")}>
          <TabsList aria-label="Calendar source">
            <TabsTrigger value="file" disabled={busy}>Local file</TabsTrigger>
            <TabsTrigger value="subscription" disabled={busy}>Subscription</TabsTrigger>
          </TabsList>
          <TabsContent value="file" className="space-y-2 pt-2">
            <label htmlFor="ical-file" className="text-sm font-medium">Import an .ics file</label>
            <Input
              id="ical-file" type="file" accept=".ics,text/calendar" disabled={busy}
              onChange={e => {
                const file = e.currentTarget.files?.[0];
                e.currentTarget.value = "";
                if (file) return add(() => importICalendarFile(file), "Could not import this calendar file.");
              }}
            />
            <p className="text-xs text-muted-foreground">The imported copy stays on this device.</p>
          </TabsContent>
          <TabsContent value="subscription" className="pt-2">
            <form
              className="space-y-3" autoComplete="off"
              onSubmit={e => {
                e.preventDefault();
                if (name.trim() && url.trim()) {
                  return add(() => addICalendarSubscription(name.trim(), url.trim()), "Could not add this subscription.");
                }
              }}
            >
              <div className="space-y-1">
                <label htmlFor="ical-name" className="text-sm font-medium">Calendar name</label>
                <Input id="ical-name" type="text" required maxLength={80}
                  value={name} disabled={busy} onChange={e => setName(e.target.value)} />
              </div>
              <div className="space-y-1">
                <label htmlFor="ical-url" className="text-sm font-medium">Subscription URL</label>
                <Input id="ical-url" type="password" inputMode="url" autoComplete="off" spellCheck={false} required
                  value={url} disabled={busy} onChange={e => setUrl(e.target.value)} />
              </div>
              <Button type="submit" disabled={busy || !name.trim() || !url.trim()}>Add subscription</Button>
            </form>
          </TabsContent>
        </Tabs>
        {busy && <p role="status" className="text-sm text-muted-foreground">Adding calendar…</p>}
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      </DialogContent>
    </Dialog>
  );
}

const dateFormat = new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric" });
const timeFormat = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const EVENTS_PER_PAGE = 50;

function eventTime(event: ICalendarEvent) {
  if (!event.start) return "";
  const start = new Date(event.start);
  if (Number.isNaN(start.getTime())) return "";
  return [dateFormat.format(start), event.allDay ? "All day" : timeFormat.format(start)].join(" · ");
}

function attendeeKey(attendee: ICalendarEvent["attendees"][number]) {
  return JSON.stringify([attendee.name, attendee.email]);
}

function EventReview({ event, pending }: { event: ICalendarEvent; pending: MeetingCalendarLink | null }) {
  const [edits, setEdits] = useState<Record<string, boolean>>({});
  const [saveFailed, setSaveFailed] = useState(false);
  const saved = pending?.source === "ical" && pending.sourceId === event.sourceId && pending.eventId === event.id
    ? pending : null;
  const savedChoices = new Map(saved?.attendees.map(a => [attendeeKey(a), a.included]));
  const attendees = event.attendees.map(attendee => {
    const key = attendeeKey(attendee);
    return { ...attendee, included: edits[key] ?? savedChoices.get(key) ?? true };
  });
  const confirmed = !!saved && saved.subject === event.subject && saved.attendees.length === attendees.length
    && attendees.every(a => savedChoices.get(attendeeKey(a)) === a.included);

  return (
    <div className="space-y-3">
      {attendees.length > 0 && (
        <fieldset className="space-y-1">
          <legend className="mb-1 text-sm font-medium">Invited attendees</legend>
          {attendees.map((attendee, index) => {
            const key = attendeeKey(attendee);
            return (
              <label key={key + index} className="flex items-center gap-2 text-sm">
                <Checkbox checked={attendee.included}
                  onCheckedChange={value => setEdits(previous => ({ ...previous, [key]: value === true }))} />
                <span className="break-words">{attendee.name || attendee.email}</span>
              </label>
            );
          })}
        </fieldset>
      )}
      <Button type="button" size="sm" disabled={confirmed}
        onClick={() => {
          const stored = setPendingCalendar({ eventId: event.id, source: "ical", sourceId: event.sourceId, subject: event.subject, attendees });
          setSaveFailed(!stored);
        }}>
        {confirmed ? "Set for next recording" : "Use for next recording"}
      </Button>
      {saveFailed && !confirmed && <p role="alert" className="text-sm text-destructive">
        Could not save the meeting selection. Check access to app storage and try again.
      </p>}
    </div>
  );
}

export function ICalendarSettings() {
  const { sources, events, issues, loading, error, refresh } = useICalendar();
  const pending = usePendingCalendar();
  const [selectedId, setSelectedId] = useState("");
  const [removing, setRemoving] = useState<string | null>(null);
  const [actionError, setActionError] = useState("");
  const selected = events.find(event => event.id === selectedId)
    ?? events.find(event => pending?.source === "ical" && pending.eventId === event.id)
    ?? events[0];
  const pageStart = selected ? Math.floor(events.indexOf(selected) / EVENTS_PER_PAGE) * EVENTS_PER_PAGE : 0;
  const page = events.slice(pageStart, pageStart + EVENTS_PER_PAGE);
  const sourceNames = new Map(sources.map(source => [source.id, source.name]));

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

  return (
    <PageSection title="iCalendar" description="Choose meetings from .ics files and read-only calendar subscriptions."
      actions={<AddCalendarDialog />}>
      <div className="space-y-4 p-5">
        {sources.length > 0 && (
          <ul aria-label="Calendars" className="divide-y divide-border">
            {sources.map(source => (
              <li key={source.id} className="flex items-center justify-between gap-3 py-2 first:pt-0">
                <div className="min-w-0">
                  <p className="break-words text-sm font-medium">{source.name}</p>
                  <p className="text-xs text-muted-foreground">{source.kind === "subscription" ? "Subscription" : "Imported file"}</p>
                  {issues[source.id] && <p className="mt-1 text-xs text-destructive">{issues[source.id]}</p>}
                </div>
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
              </li>
            ))}
          </ul>
        )}
        {(error || actionError) && <p role="alert" className="text-sm text-destructive">{actionError || error}</p>}
        {(sources.length > 0 || error) && (
          <div className="flex items-center justify-between gap-2">
            <h3 className="text-sm font-semibold">Upcoming meetings</h3>
            <Button size="sm" variant="ghost" disabled={loading || Boolean(removing)}
              onClick={() => {
                setActionError("");
                void refresh().catch(() => setActionError("Could not refresh calendars. Try again."));
              }}>
              <RefreshCw aria-hidden="true" className={loading ? "animate-spin" : ""} />Refresh
            </Button>
          </div>
        )}
        {(loading || removing) && <p role="status" className="text-sm text-muted-foreground">
          {removing ? "Removing calendar…" : "Loading calendars…"}
        </p>}
        {!loading && !error && !events.length && (!sources.length || sources.some(s => !issues[s.id])) && <p className="text-sm text-muted-foreground">
          {sources.length ? "No meetings found in the next 7 days." : "No calendars added."}
        </p>}
        {selected && (
          <div className="space-y-3">
            <Select value={selected.id} onValueChange={setSelectedId}>
              <SelectTrigger aria-label="Upcoming meeting"><SelectValue /></SelectTrigger>
              <SelectContent>
                {page.map(event => (
                  <SelectItem key={event.id} value={event.id}>
                    {[event.subject || "Untitled meeting", eventTime(event), sourceNames.get(event.sourceId)].filter(Boolean).join(" · ")}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {events.length > EVENTS_PER_PAGE && (
              <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                <span>Meetings {pageStart + 1}–{pageStart + page.length} of {events.length}</span>
                <Button type="button" size="sm" variant="ghost" disabled={pageStart === 0}
                  onClick={() => setSelectedId(events[pageStart - EVENTS_PER_PAGE].id)}>Previous</Button>
                <Button type="button" size="sm" variant="ghost" disabled={pageStart + page.length >= events.length}
                  onClick={() => setSelectedId(events[pageStart + page.length].id)}>Next</Button>
              </div>
            )}
            <EventReview key={selected.id} event={selected} pending={pending} />
          </div>
        )}
      </div>
    </PageSection>
  );
}
