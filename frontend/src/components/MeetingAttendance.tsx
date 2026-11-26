"use client";

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CalendarClock } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger,
} from "@/components/ui/dialog";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import {
  ACTIVE_CALENDAR_CHANGED, getActiveRecordingCalendar, MICROSOFT_CALENDAR_CLEARED,
  microsoftCalendarRevision, sameCalendarEvent, setActiveRecordingCalendar, type MeetingCalendarLink,
} from "@/lib/meetingCalendar";
import { readRecordingCalendar, writeRecordingCalendar } from "@/services/recordingCalendarService";
import {
  getMeetingCalendar, setMeetingCalendar, MEETING_SERIES_TEMPLATE_CHANGED,
} from "@/lib/meetingCalendarStore";
import { findCalendarEventsBetween, findOngoingRecordingCalendar } from "@/services/recordingCalendarService";
import { archiveDateKey, parseArchiveDate } from "@/lib/archive-review";
import type { RecordingState } from "@/services/recordingService";

const RELINK_MARGIN_MS = 10 * 60_000;

const eventTitle = (link: MeetingCalendarLink) => link.subject || "Untitled calendar event";

/** Events from the calendars connected right now, kept apart from the record saved with the meeting. */
interface CandidateLookup {
  loading: boolean;
  unavailable: string[];
  onRetry: () => void;
  date?: string;
  onDateChange?: (date: string) => void;
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
            {link ? "Check the people who attended." : "Choose the event this recording belongs to."}
            {" "}Saving lets your AI provider use the event title and checked names when you summarize. Email addresses stay on this device.
          </DialogDescription>
        </DialogHeader>
        {lookup?.onDateChange && <div className="space-y-1">
          <Label htmlFor="calendar-search-date">Calendar date</Label>
          <Input id="calendar-search-date" type="date" value={lookup.date || ""}
            disabled={saving || lookup.loading} onChange={event => {
              setDraft(null);
              lookup.onDateChange?.(event.target.value);
            }} />
        </div>}
        {lookup?.loading && options.length === 0 && <p role="status" className="text-sm text-muted-foreground">Looking for calendar events…</p>}
        {lookup && !lookup.loading && !link && options.length === 0 && lookup.unavailable.length === 0 && (
          <p role="status" className="text-sm text-muted-foreground">{lookup.onDateChange
            ? "No calendar events found for this date." : "No calendar events found around this recording."}</p>
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
                  {option.start && ` · ${new Date(option.start).toLocaleString([], {
                    ...(lookup?.onDateChange ? { month: "short", day: "numeric" } as const : {}),
                    hour: "numeric", minute: "2-digit",
                  })}`}
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

export function RecordingCalendarReview({ disabled, sessionId }: {
  disabled: boolean;
  sessionId: string;
}) {
  const [active, setActive] = useState(() => getActiveRecordingCalendar(sessionId));
  const [loadFailed, setLoadFailed] = useState(false);
  const [checking, setChecking] = useState(false);
  const [loadAttempt, setLoadAttempt] = useState(0);
  const [candidates, setCandidates] = useState({ loading: false, matches: [] as MeetingCalendarLink[], unavailable: [] as string[] });
  const lookup = useRef(0);
  const openedMicrosoftRevision = useRef(microsoftCalendarRevision);
  useEffect(() => {
    let disposed = false;
    const sync = () => {
      setActive(getActiveRecordingCalendar(sessionId));
      if (getActiveRecordingCalendar(sessionId)) setLoadFailed(false);
    };
    const hydrate = async (refresh = false) => {
      const before = getActiveRecordingCalendar(sessionId);
      if (before && !refresh) return;
      try {
        const draft = await readRecordingCalendar(sessionId);
        if (disposed || getActiveRecordingCalendar(sessionId) !== before) return;
        setActiveRecordingCalendar({ sessionId, status: "none", matches: [], ...before, ...draft });
      } catch {
        if (!disposed && !getActiveRecordingCalendar(sessionId)) setLoadFailed(true);
      }
    };
    const signOut = () => {
      lookup.current++;
      setCandidates(current => ({ ...current, loading: false, matches: current.matches.filter(event => event.source !== "microsoft") }));
      void hydrate(true);
    };
    window.addEventListener(ACTIVE_CALENDAR_CHANGED, sync);
    window.addEventListener(MICROSOFT_CALENDAR_CLEARED, signOut);
    sync();
    void hydrate();
    return () => {
      disposed = true;
      lookup.current++;
      window.removeEventListener(ACTIVE_CALENDAR_CHANGED, sync);
      window.removeEventListener(MICROSOFT_CALENDAR_CLEARED, signOut);
    };
  }, [sessionId, loadAttempt]);
  if (loadFailed) return <Button variant="outline" size="sm" disabled={disabled}
    onClick={() => { setLoadFailed(false); setLoadAttempt(attempt => attempt + 1); }}>Retry loading recording calendar</Button>;
  if (!active || active.sessionId !== sessionId) return null;
  const status = active.link ? eventTitle(active.link)
    : active.status === "ambiguous" ? "Multiple calendar events match this recording"
    : active.status === "upcoming" ? "A calendar event starts in the next five minutes"
    : active.status === "unavailable"
      ? active.unavailable?.length ? `Could not read ${active.unavailable.join(" and ")}` : "Some calendar information was unavailable"
    : active.matches.length > 0 ? "Found a matching calendar event"
    : "No calendar event linked";
  // A partial result is not final: look again for the sources that failed.
  async function checkAgain() {
    if (checking || !active) return;
    setChecking(true);
    try {
      const match = await findOngoingRecordingCalendar(active.startedAt ?? Date.now());
      // Ignore a result that arrives after the person or the recording changed the selection.
      if (getActiveRecordingCalendar(sessionId) === active) setActiveRecordingCalendar({ ...active, ...match, link: active.link });
    } catch { /* The status stays as it was. */ } finally {
      setChecking(false);
    }
  }
  async function findCandidates() {
    const version = ++lookup.current;
    const microsoftRevision = microsoftCalendarRevision;
    const end = Date.now();
    setCandidates(current => ({ ...current, loading: true }));
    try {
      let start = active?.startedAt;
      if (start === undefined) {
        const snapshot = await invoke<RecordingState>("get_recording_state").catch(() => null);
        if (snapshot?.session_id === sessionId && typeof snapshot.recording_duration === "number" &&
            Number.isFinite(snapshot.recording_duration) && snapshot.recording_duration >= 0) {
          start = end - snapshot.recording_duration * 1000;
        }
      }
      if (version !== lookup.current) return;
      const result = await findCalendarEventsBetween(Math.min(start ?? end, end) - RELINK_MARGIN_MS, end + RELINK_MARGIN_MS);
      if (version === lookup.current && microsoftRevision === microsoftCalendarRevision) {
        setCandidates({ loading: false, ...result });
      }
    } catch {
      if (version === lookup.current) setCandidates({ loading: false, matches: [], unavailable: ["your calendars"] });
    }
  }
  const possibleEvents = [active.link, ...active.matches, ...candidates.matches]
    .filter((event): event is MeetingCalendarLink => event !== null)
    .filter((event, index, events) => events.findIndex(other => sameCalendarEvent(other, event)) === index);
  return (
    <div className="flex flex-wrap items-center justify-center gap-x-3 gap-y-1">
      <span role="status" className="min-w-0 break-words text-sm text-muted-foreground">{status}</span>
      {!active.link && active.status === "unavailable" && (
        <Button variant="ghost" size="sm" disabled={disabled || checking} onClick={() => void checkAgain()}>
          {checking ? "Checking…" : "Check again"}
        </Button>
      )}
        <AttendanceDialog link={active.link} options={possibleEvents}
          label={active.link ? "Review attendance" : "Link calendar event"} disabled={disabled}
          onOpen={() => {
            openedMicrosoftRevision.current = microsoftCalendarRevision;
            void findCandidates();
          }}
          lookup={{ loading: candidates.loading, unavailable: candidates.unavailable, onRetry: () => void findCandidates() }}
          onSave={async link => {
            if (link?.source === "microsoft" && openedMicrosoftRevision.current !== microsoftCalendarRevision) throw new Error("Microsoft calendar changed.");
            if (getActiveRecordingCalendar(sessionId) !== active) throw new Error("The recording calendar changed.");
            const revision = await writeRecordingCalendar(sessionId, active.revision, link);
            if (getActiveRecordingCalendar(sessionId) === active) {
              setActiveRecordingCalendar({ ...active, link, revision, ...(link ? {} : { status: "none", matches: [] }) });
            }
          }} />
    </div>
  );
}

export function SavedMeetingAttendance({ meetingId, createdAt, durationSeconds, disabled }: {
  meetingId: string;
  /** The capture, import or legacy save date. */
  createdAt: string;
  durationSeconds?: number;
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
  const [overrideDate, setOverrideDate] = useState("");
  const recordingDate = archiveDateKey(createdAt) ?? "";
  const findCandidates = async (date = overrideDate) => {
    const version = ++lookup.current;
    const start = parseArchiveDate(date || recordingDate);
    if (!start) {
      setCandidates({ loading: false, matches: [], unavailable: [] });
      return;
    }
    start.setHours(0, 0, 0, 0);
    const end = new Date(start);
    end.setDate(end.getDate() + 1);
    const recordedAt = date || createdAt === recordingDate ? null : parseArchiveDate(createdAt);
    const duration = Number.isFinite(durationSeconds) && durationSeconds !== undefined
      ? Math.min(86_400, Math.max(900, durationSeconds))
      : 3_600;
    // Legacy meetings may have stored the save time after a recording crossed midnight.
    const from = recordedAt
      ? Math.min(start.getTime(), recordedAt.getTime() - duration * 1000 - RELINK_MARGIN_MS)
      : start.getTime();
    const to = recordedAt ? Math.max(end.getTime(), recordedAt.getTime() + RELINK_MARGIN_MS) : end.getTime();
    setCandidates({ loading: true, matches: [], unavailable: [] });
    try {
      const result = await findCalendarEventsBetween(from, to, true);
      if (version === lookup.current) setCandidates({ loading: false, ...result });
    } catch {
      if (version === lookup.current) setCandidates({ loading: false, matches: [], unavailable: ["your calendars"] });
    }
  };
  const load = useCallback(async () => {
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
  }, [meetingId]);
  useEffect(() => {
    void load();
    const sync = () => {
      lookup.current++;
      setCandidates(current => ({
        ...current, loading: false, matches: current.matches.filter(event => event.source !== "microsoft"),
      }));
      void load();
    };
    window.addEventListener(MICROSOFT_CALENDAR_CLEARED, sync);
    return () => { request.current += 1; lookup.current += 1; window.removeEventListener(MICROSOFT_CALENDAR_CLEARED, sync); };
  }, [load]);
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
        lookup={{
          loading: candidates.loading,
          unavailable: candidates.unavailable,
          date: overrideDate || recordingDate,
          onRetry: () => void findCandidates(),
          onDateChange: value => { setOverrideDate(value); void findCandidates(value); },
        }} />
    );
  }
  return (
    <AttendanceDialog link={link} label="Attendance" disabled={disabled} retained onSave={save} />
  );
}
