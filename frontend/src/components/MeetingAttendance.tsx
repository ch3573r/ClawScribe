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
import { findCalendarEventsBetween, findOngoingRecordingCalendar } from "@/services/recordingCalendarService";
import type { TemplateInfo } from "@/hooks/meeting-details/useTemplates";

const eventTitle = (link: MeetingCalendarLink) => link.subject || "Untitled calendar event";

/** Events from the calendars connected right now, kept apart from the record saved with the meeting. */
interface CandidateLookup {
  loading: boolean;
  unavailable: string[];
  onRetry: () => void;
}

function UnavailableSources({ lookup }: { lookup: CandidateLookup }) {
  if (lookup.loading || lookup.unavailable.length === 0) return null;
  return (
    <div role="status" className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
      <span className="min-w-0 break-words">
        Could not read {lookup.unavailable.join(" and ")}. Events from there may be missing.
      </span>
      <Button type="button" variant="outline" size="sm" onClick={lookup.onRetry}>Check again</Button>
    </div>
  );
}

function AttendanceDialog({ link, options = [], label, onSave, disabled, children, lookup, onOpen, retained }: {
  link: MeetingCalendarLink | null;
  options?: MeetingCalendarLink[];
  label: string;
  onSave: (link: MeetingCalendarLink | null) => Promise<void>;
  disabled?: boolean;
  children?: ReactNode;
  lookup?: CandidateLookup;
  onOpen?: () => void;
  /** The link is saved with a meeting, so it outlives a Microsoft 365 sign-out. */
  retained?: boolean;
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
    if (value) onOpen?.();
  }

  async function save(value: MeetingCalendarLink | null) {
    if (pending.current || disabled) return;
    pending.current = true;
    setSaving(true);
    setFailed(false);
    try {
      // Saving the dialog is the review: it releases the title and checked names to summaries.
      await onSave(value ? { ...value, reviewed: true } : null);
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
            {link ? "Check the people who attended. Saving shares the event title with your AI provider, and the names of checked people when you summarize. Email addresses stay on this device."
              : "Choose the event this recording belongs to."}
          </DialogDescription>
        </DialogHeader>
        {lookup?.loading && options.length === 0 && <p role="status" className="text-sm text-muted-foreground">Looking for calendar events…</p>}
        {lookup && !lookup.loading && !link && options.length === 0 && lookup.unavailable.length === 0 && (
          <p role="status" className="text-sm text-muted-foreground">No calendar events found around this recording.</p>
        )}
        {lookup && <UnavailableSources lookup={lookup} />}
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
            {draft.attendees.some(person => !person.name && person.email) && (
              <p className="text-xs text-muted-foreground">People shown by email address have no name and are never included in summaries.</p>
            )}
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
        {retained && link && (
          <p className="text-xs text-muted-foreground">
            {link.source === "microsoft"
              ? "Saved with this meeting on this device. Signing out of Microsoft 365 does not remove it. Unlink the event to remove it."
              : "Saved with this meeting on this device. Unlink the event to remove it."}
          </p>
        )}
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
  const [checking, setChecking] = useState(false);
  useEffect(() => {
    const sync = () => setActive(getActiveRecordingCalendar());
    window.addEventListener(ACTIVE_CALENDAR_CHANGED, sync);
    sync();
    return () => window.removeEventListener(ACTIVE_CALENDAR_CHANGED, sync);
  }, []);
  if (!active || (!active.link && active.status === "none")) return null;
  const status = active.link ? eventTitle(active.link)
    : active.status === "ambiguous" ? "Multiple ongoing calendar events"
    : active.status === "unavailable"
      ? active.unavailable?.length ? `Could not read ${active.unavailable.join(" and ")}` : "Some calendar information was unavailable"
    : active.matches.length > 0 ? "Found an ongoing calendar event"
    : "No calendar event linked";
  // A partial result is not final: look again for the sources that failed.
  async function checkAgain() {
    if (checking || !active) return;
    setChecking(true);
    try {
      const match = await findOngoingRecordingCalendar(Date.now());
      // Ignore a result that arrives after the person or the recording changed the selection.
      if (getActiveRecordingCalendar() === active) setActiveRecordingCalendar({ ...active, ...match, link: active.link });
    } catch { /* The status stays as it was. */ } finally {
      setChecking(false);
    }
  }
  return (
    <div className="flex flex-wrap items-center justify-center gap-x-3 gap-y-1">
      <span role="status" className="min-w-0 break-words text-sm text-muted-foreground">{status}</span>
      {!active.link && active.status === "unavailable" && (
        <Button variant="ghost" size="sm" disabled={disabled || checking} onClick={() => void checkAgain()}>
          {checking ? "Checking…" : "Check again"}
        </Button>
      )}
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

/** Time around a saved recording in which a calendar event can still be the one it belongs to. */
const RELINK_MARGIN_MS = 10 * 60_000;

export function SavedMeetingAttendance({ meetingId, createdAt, durationSeconds, templates, remembered, disabled }: {
  meetingId: string;
  /** The save time of the recording; events are searched before it for the recorded duration. */
  createdAt: string;
  durationSeconds?: number;
  templates: TemplateInfo[];
  remembered: string | null;
  disabled?: boolean;
}) {
  const [link, setLink] = useState<MeetingCalendarLink | null>(null);
  const [failed, setFailed] = useState(false);
  const [loading, setLoading] = useState(true);
  const [candidates, setCandidates] = useState<{ loading: boolean; matches: MeetingCalendarLink[]; unavailable: string[] }>(
    { loading: false, matches: [], unavailable: [] },
  );
  const request = useRef(0);
  const lookup = useRef(0);
  const findCandidates = async () => {
    const version = ++lookup.current;
    const end = new Date(createdAt).getTime();
    if (!Number.isFinite(end)) {
      setCandidates({ loading: false, matches: [], unavailable: [] });
      return;
    }
    setCandidates(current => ({ ...current, loading: true }));
    const from = end - Math.max(durationSeconds ?? 3_600, 900) * 1000 - RELINK_MARGIN_MS;
    try {
      const result = await findCalendarEventsBetween(from, end + RELINK_MARGIN_MS);
      if (version === lookup.current) setCandidates({ loading: false, ...result });
    } catch {
      if (version === lookup.current) setCandidates({ loading: false, matches: [], unavailable: ["your calendars"] });
    }
  };
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
    return () => { request.current++; lookup.current++; window.removeEventListener("clawscribe-microsoft-calendar-cleared", sync); };
    // The parent keys this control by meeting ID.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [meetingId]);
  if (loading) return <span role="status" className="text-xs text-muted-foreground">Loading attendance…</span>;
  if (failed) return <Button variant="outline" size="sm" onClick={() => void load()}>Retry loading attendance</Button>;
  const save = async (value: MeetingCalendarLink | null) => {
    await setMeetingCalendar(meetingId, value);
    setLink(value);
    // The remembered template follows the event's series.
    window.dispatchEvent(new Event(MEETING_SERIES_TEMPLATE_CHANGED));
  };
  // An unlinked or restored meeting can be linked again. These events come from the calendars connected now.
  if (!link) {
    return (
      <AttendanceDialog link={null} label="Link calendar event" options={candidates.matches} disabled={disabled}
        onOpen={() => void findCandidates()} onSave={save}
        lookup={{ loading: candidates.loading, unavailable: candidates.unavailable, onRetry: () => void findCandidates() }} />
    );
  }
  return (
    <AttendanceDialog link={link} label="Attendance" disabled={disabled} retained onSave={save}>
      {link.seriesId && <SeriesTemplatePreference meetingId={meetingId} templates={templates} remembered={remembered} />}
    </AttendanceDialog>
  );
}
