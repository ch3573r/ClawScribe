"use client";

import { useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CalendarClock } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger,
} from "@/components/ui/dialog";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import {
  ACTIVE_CALENDAR_CHANGED, getActiveRecordingCalendar, type MeetingCalendarLink,
  sameCalendarEvent, setActiveRecordingCalendar,
} from "@/lib/meetingCalendar";
import {
  getMeetingCalendar, setMeetingCalendar, MEETING_SERIES_TEMPLATE_CHANGED,
} from "@/lib/meetingCalendarStore";
import type { TemplateInfo } from "@/hooks/meeting-details/useTemplates";

const eventTitle = (link: MeetingCalendarLink) => link.subject || "Untitled calendar event";

function AttendanceDialog({ link, options = [], label, onSave, disabled, children }: {
  link: MeetingCalendarLink | null;
  options?: MeetingCalendarLink[];
  label: string;
  onSave: (link: MeetingCalendarLink | null) => Promise<void>;
  disabled?: boolean;
  children?: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState(link);
  const [saving, setSaving] = useState(false);
  const [failed, setFailed] = useState(false);
  const pending = useRef(false);

  function changeOpen(value: boolean) {
    if (pending.current) return;
    setOpen(value);
    setDraft(link);
    setFailed(false);
  }

  async function save(value: MeetingCalendarLink | null) {
    if (pending.current || disabled) return;
    pending.current = true;
    setSaving(true);
    setFailed(false);
    try {
      await onSave(value);
      setOpen(false);
    } catch {
      setFailed(true);
    } finally {
      pending.current = false;
      setSaving(false);
    }
  }

  const selected = draft ? options.findIndex(option => sameCalendarEvent(draft, option)) : -1;
  return (
    <Dialog open={open && !disabled} onOpenChange={changeOpen}>
      <DialogTrigger asChild>
        <Button variant="outline" size="sm" disabled={disabled}>
          <CalendarClock aria-hidden="true" />{label}
        </Button>
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{link ? "Review attendance" : "Link calendar event"}</DialogTitle>
          <DialogDescription>
            {link ? "Check the people who attended. Only checked names are included as calendar context when you generate a summary."
              : "Choose the event this recording belongs to."}
          </DialogDescription>
        </DialogHeader>
        {options.length > 0 && (!link || options.length > 1) ? (
          <Select value={selected >= 0 ? String(selected) : ""} disabled={saving}
            onValueChange={value => {
              const option = options[Number(value)];
              setDraft(sameCalendarEvent(link, option) ? link : option);
            }}>
            <SelectTrigger aria-label="Calendar event"><SelectValue placeholder="Choose an event" /></SelectTrigger>
            <SelectContent>
              {options.map((option, index) => (
                <SelectItem key={index} value={String(index)}>
                  {eventTitle(option)} · {option.source === "microsoft" ? "Microsoft 365" : "iCalendar"}
                  {option.start && ` · ${new Date(option.start).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}`}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        ) : draft && <p className="break-words text-sm font-medium">{eventTitle(draft)}</p>}
        {draft && (draft.attendees.length > 0 ? (
          <fieldset disabled={saving} className="max-h-56 space-y-2 overflow-y-auto">
            <legend className="mb-2 text-sm font-medium">Invited people</legend>
            {draft.attendees.map((person, index) => {
              const name = person.name || person.email;
              if (!name) return null;
              return (
                <label key={index} className="flex items-start gap-2 text-sm">
                  <Checkbox disabled={saving} checked={person.attendance === "present"} onCheckedChange={checked => {
                    setDraft(current => current && ({
                      ...current,
                      attendees: current.attendees.map((entry, i) => i === index
                        ? { ...entry, attendance: checked === true ? "present" : "absent" }
                        : entry),
                    }));
                  }} />
                  <span className="min-w-0 break-words">{name}</span>
                </label>
              );
            })}
          </fieldset>
        ) : <p className="text-sm text-muted-foreground">The event has no invitees available to review.</p>)}
        {children}
        {failed && <p role="alert" className="text-sm text-error-foreground">Could not save the calendar changes. Try again.</p>}
        <DialogFooter>
          {link && <Button variant="ghost" disabled={saving} onClick={() => void save(null)}>Unlink event</Button>}
          <Button disabled={saving || !draft} onClick={() => void save(draft)}>
            {saving ? "Saving…" : link ? "Save attendance" : "Link event"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function RecordingCalendarReview({ disabled, onTitleChange }: {
  disabled: boolean;
  onTitleChange: (title: string) => void;
}) {
  const [active, setActive] = useState(getActiveRecordingCalendar);
  useEffect(() => {
    const sync = () => setActive(getActiveRecordingCalendar());
    window.addEventListener(ACTIVE_CALENDAR_CHANGED, sync);
    sync();
    return () => window.removeEventListener(ACTIVE_CALENDAR_CHANGED, sync);
  }, []);
  if (!active || (!active.link && active.status === "none")) return null;
  const status = active.link ? eventTitle(active.link)
    : active.status === "ambiguous" ? "Multiple ongoing calendar events"
    : active.status === "unavailable" ? "Some calendar information was unavailable"
    : "No calendar event linked";
  return (
    <div className="flex flex-wrap items-center justify-center gap-x-3 gap-y-1">
      <span role="status" className="min-w-0 break-words text-sm text-muted-foreground">{status}</span>
      {(active.link || active.matches.length > 0) && (
        <AttendanceDialog link={active.link} options={active.matches}
          label={active.link ? "Review attendance" : "Choose event"} disabled={disabled}
          onSave={async link => {
            if (getActiveRecordingCalendar() !== active) throw new Error("The recording calendar changed.");
            setActiveRecordingCalendar({ ...active, link, ...(link ? {} : { status: "none", matches: [] }) });
            onTitleChange(link?.subject?.trim() || active.fallbackTitle);
          }} />
      )}
    </div>
  );
}

function SeriesTemplatePreference({ meetingId, templates, remembered }: {
  meetingId: string;
  templates: TemplateInfo[];
  remembered: string | null;
}) {
  const [saving, setSaving] = useState(false);
  return (
    <div className="space-y-1">
      <span className="text-sm font-medium">Template for this series</span>
      <Select value={remembered ?? "use-default"} disabled={saving}
        onValueChange={async value => {
          setSaving(true);
          try {
            await invoke("set_meeting_series_template", { meetingId, templateId: value === "use-default" ? null : value });
            window.dispatchEvent(new Event(MEETING_SERIES_TEMPLATE_CHANGED));
          } catch {
            toast.error("Could not save the series template.");
          } finally {
            setSaving(false);
          }
        }}>
        <SelectTrigger aria-label="Template for this series"><SelectValue /></SelectTrigger>
        <SelectContent>
          <SelectItem value="use-default">Use the default template</SelectItem>
          {templates.map(template => <SelectItem key={template.id} value={template.id}>{template.name}</SelectItem>)}
        </SelectContent>
      </Select>
    </div>
  );
}

export function SavedMeetingAttendance({ meetingId, templates, remembered, disabled }: {
  meetingId: string;
  templates: TemplateInfo[];
  remembered: string | null;
  disabled?: boolean;
}) {
  const [link, setLink] = useState<MeetingCalendarLink | null>(null);
  const [failed, setFailed] = useState(false);
  const [loading, setLoading] = useState(true);
  const request = useRef(0);
  const load = async () => {
    const version = ++request.current;
    setLoading(true);
    setFailed(false);
    try {
      const result = await getMeetingCalendar(meetingId);
      if (version === request.current) setLink(result);
    } catch {
      if (version === request.current) setFailed(true);
    } finally {
      if (version === request.current) setLoading(false);
    }
  };
  useEffect(() => {
    void load();
    const sync = () => { void load(); };
    window.addEventListener("clawscribe-microsoft-calendar-cleared", sync);
    return () => { request.current++; window.removeEventListener("clawscribe-microsoft-calendar-cleared", sync); };
    // The parent keys this control by meeting ID.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [meetingId]);
  if (loading) return <span role="status" className="text-xs text-muted-foreground">Loading attendance…</span>;
  if (failed) return <Button variant="outline" size="sm" onClick={() => void load()}>Retry loading attendance</Button>;
  if (!link) return null;
  return (
    <AttendanceDialog link={link} label="Attendance" disabled={disabled} onSave={async value => {
      await setMeetingCalendar(meetingId, value);
      setLink(value);
      if (!value) window.dispatchEvent(new Event(MEETING_SERIES_TEMPLATE_CHANGED));
    }}>
      {link.seriesId && <SeriesTemplatePreference meetingId={meetingId} templates={templates} remembered={remembered} />}
    </AttendanceDialog>
  );
}
