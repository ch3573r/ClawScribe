"use client";

import { useRef, useState } from "react";
import { Plus } from "lucide-react";
import { toast } from "sonner";
import { calendarLinkFromEvent, sameCalendarEvent, setPendingCalendar, type MeetingCalendarLink } from "@/lib/meetingCalendar";
import { addICalendarSubscription, importICalendarFile } from "@/services/icalService";
import { Button } from "@/components/ui/button";
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";

/** One upcoming meeting from any calendar source, in the shape the Calendars page lists. */
export interface UpcomingEvent {
  key: string;
  source: "microsoft" | "ical";
  sourceId?: string;
  seriesId?: string;
  organizer?: { name: string | null; email: string | null } | null;
  sourceName: string;
  id: string;
  subject: string | null;
  start: string | null;
  allDay: boolean;
  attendees: { name: string | null; email: string | null }[];
}

export function AddCalendarDialog() {
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
      <DialogTrigger asChild><Button variant="outline" size="sm"><Plus aria-hidden="true" />Add calendar</Button></DialogTrigger>
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
        {error && <p role="alert" className="text-sm text-error-foreground">{error}</p>}
      </DialogContent>
    </Dialog>
  );
}

const dayFormat = new Intl.DateTimeFormat(undefined, { weekday: "short", day: "numeric", month: "short" });
const timeFormat = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });
const startOfDay = (date: Date) => new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();

/** "Today 13:00", "Tomorrow 09:30", "Mon 23 Nov 10:00" or "Mon 23 Nov All day". */
export function formatWhen(start: string | null, allDay = false, now = new Date()): string {
  if (!start) return "Time unknown";
  const date = new Date(start);
  if (Number.isNaN(date.getTime())) return "Time unknown";
  const days = Math.round((startOfDay(date) - startOfDay(now)) / 86_400_000);
  const day = days === 0 ? "Today" : days === 1 ? "Tomorrow" : dayFormat.format(date).replace(",", "");
  return `${day} ${allDay ? "All day" : timeFormat.format(date)}`;
}

export function UseEventButton({ event, pending }: { event: UpcomingEvent; pending: MeetingCalendarLink | null }) {
  const link = calendarLinkFromEvent(event);
  const selected = sameCalendarEvent(pending, link);
  const title = event.subject || "Untitled meeting";
  return (
    <Button type="button" variant="ghost" size="sm" disabled={selected}
      aria-label={selected ? `Selected ${title} for next recording` : `Use ${title} for next recording`}
      onClick={() => {
        if (!setPendingCalendar(link)) toast.error("Could not save the meeting selection on this device.");
      }}>
      {selected ? "Selected" : "Use"}
    </Button>
  );
}
