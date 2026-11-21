"use client";

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import Link from "next/link";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { formatArchiveDate } from "@/lib/archive-review";
import { type MeetingCalendarLink, MICROSOFT_CALENDAR_CLEARED } from "@/lib/meetingCalendar";
import { meetingActionUrl, SERIES_REVIEW_CHANGED, type FollowUpDestination, type SeriesOccurrence, type SeriesPreferences } from "@/lib/meeting-follow-through";
import { LANGUAGE_OPTIONS } from "@/lib/summary-languages";

interface SeriesReview {
  subject: string | null;
  reviewToken: string;
  preferences: SeriesPreferences;
  approvedNotes: string;
  priorOccurrences: SeriesOccurrence[];
}

function PreparationBrief({ occurrences, close }: { occurrences: SeriesOccurrence[]; close: () => void }) {
  if (!occurrences.length) return <p className="text-sm text-muted-foreground">No prior reviewed notes or open actions found for this series.</p>;
  return <section aria-label="Prior reviewed occurrences" className="space-y-3">
    {occurrences.map(occurrence => <article key={occurrence.meetingId} className="space-y-2 rounded-md border border-border p-3">
      <h3 className="break-words text-sm font-medium">
        <Link className="rounded-sm text-primary underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          onClick={close} href={`/meeting-details?${new URLSearchParams({ id: occurrence.meetingId, archiveAction: "series" })}`}>
          {occurrence.title} · {formatArchiveDate(occurrence.startsAt)}
        </Link>
      </h3>
      {occurrence.approvedNotes && <p className="whitespace-pre-wrap break-words text-sm">{occurrence.approvedNotes}</p>}
      {!!occurrence.actions.length && <ul className="space-y-1">
        {occurrence.actions.map(action => <li key={action.id} className="flex items-start justify-between gap-2 text-sm">
          <span className="min-w-0 break-words">{action.title}{action.owner ? ` · ${action.owner}` : ""}{action.dueDate ? ` · Due ${action.dueDate}` : ""}</span>
          <Button variant="ghost" size="sm" asChild><Link onClick={close} href={meetingActionUrl(occurrence.meetingId, action.id, false)}>Review action</Link></Button>
        </li>)}
      </ul>}
    </article>)}
  </section>;
}

export function UpcomingSeriesPreparation({ event, close }: { event: MeetingCalendarLink; close: () => void }) {
  const [occurrences, setOccurrences] = useState<SeriesOccurrence[] | null>(null);
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let active = true;
    setError("");
    void invoke<SeriesOccurrence[]>("get_upcoming_series_brief", { event }).then(data => {
      if (active) setOccurrences(data);
    }).catch(() => { if (active) setError("Could not load the preparation brief."); });
    return () => { active = false; };
  }, [event, attempt]);
  useEffect(() => {
    if (event.source !== "microsoft") return;
    window.addEventListener(MICROSOFT_CALENDAR_CLEARED, close);
    return () => window.removeEventListener(MICROSOFT_CALENDAR_CLEARED, close);
  }, [close, event.source]);
  return <Dialog open onOpenChange={open => { if (!open) close(); }}>
    <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-xl">
      <DialogHeader>
        <DialogTitle>Preparation brief</DialogTitle>
        <DialogDescription>{event.subject || "Recurring meeting"} · Earlier reviewed occurrences in this series</DialogDescription>
      </DialogHeader>
      {error ? <><p role="alert" className="text-sm text-destructive">{error}</p><Button variant="outline" onClick={() => setAttempt(value => value + 1)}>Retry</Button></>
        : occurrences ? <PreparationBrief occurrences={occurrences} close={close} />
        : <p role="status" className="text-sm text-muted-foreground">Loading preparation brief…</p>}
    </DialogContent>
  </Dialog>;
}

export function SavedSeriesReview({ meetingId, close }: { meetingId: string; close: () => void }) {
  const [data, setData] = useState<SeriesReview | null | undefined>();
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  useEffect(() => {
    let active = true;
    setData(undefined);
    setError("");
    void invoke<SeriesReview | null>("get_meeting_series_review", { meetingId }).then(data => {
      if (active) setData(data);
    }).catch(() => { if (active) setError("Could not load this series."); });
    return () => { active = false; };
  }, [meetingId, attempt]);

  async function save() {
    if (!data || pending.current) return;
    pending.current = true;
    setBusy(true);
    setError("");
    try {
      const { reviewToken, approvedNotes, preferences } = data;
      await invoke("save_meeting_series_review", { meetingId, reviewToken, approvedNotes, preferences });
      window.dispatchEvent(new Event(SERIES_REVIEW_CHANGED));
      close();
    } catch (failure) {
      setError(typeof failure === "string" ? failure : "Could not save the series preferences.");
    } finally {
      pending.current = false;
      setBusy(false);
    }
  }

  function update(preferences: Partial<SeriesPreferences>) {
    setData(current => current && ({ ...current, preferences: { ...current.preferences, ...preferences } }));
  }

  return <Dialog open onOpenChange={open => { if (!open && !pending.current) close(); }}>
    <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-xl">
      <DialogHeader>
        <DialogTitle>Recurring meeting</DialogTitle>
        <DialogDescription>{data?.subject || "Review prior evidence and set defaults for this series."}</DialogDescription>
      </DialogHeader>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      {data === undefined && (error ? <Button variant="outline" onClick={() => setAttempt(value => value + 1)}>Retry</Button>
        : <p role="status" className="text-sm text-muted-foreground">Loading series…</p>)}
      {data === null && <p className="text-sm text-muted-foreground">Link a recurring event and save its attendance review to use series preferences.</p>}
      {data && <>
        <PreparationBrief occurrences={data.priorOccurrences} close={close} />
        <form className="space-y-4 border-t border-border pt-4" onSubmit={event => { event.preventDefault(); void save(); }}>
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="space-y-1">
              <Label htmlFor="series-language">Summary language</Label>
              <Select disabled={busy} value={data.preferences.language ?? "default"} onValueChange={value => update({ language: value === "default" ? null : value })}>
                <SelectTrigger id="series-language"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="default">Use meeting or automatic language</SelectItem>
                  {LANGUAGE_OPTIONS.map(language => <SelectItem key={language.code} value={language.code}>{language.label}</SelectItem>)}
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-1">
              <Label htmlFor="series-destination">Preferred follow-up</Label>
              <Select disabled={busy} value={data.preferences.destination ?? "default"} onValueChange={value => update({ destination: value === "default" ? null : value as FollowUpDestination })}>
                <SelectTrigger id="series-destination"><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem value="default">No preference</SelectItem>
                  <SelectItem value="planner">Planner</SelectItem>
                  <SelectItem value="todo">Microsoft To Do</SelectItem>
                  <SelectItem value="email">Email draft</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>
          <p className="text-xs text-muted-foreground">Choose the series template in Attendance. Preferred follow-up highlights an option in Export. Review each export or draft before opening it.</p>
          <div className="space-y-1">
            <Label htmlFor="series-note">Approved decisions and notes for this occurrence</Label>
            <Textarea id="series-note" maxLength={2000} rows={4} disabled={busy} value={data.approvedNotes}
              onChange={event => setData(current => current && ({ ...current, approvedNotes: event.target.value }))} />
            <p className="text-xs text-muted-foreground">This text stays local and can appear in later preparation briefs for this series.</p>
          </div>
          <div className="flex items-center gap-2">
            <Checkbox id="series-include-notes" disabled={busy} checked={data.preferences.includeApprovedNotes}
              onCheckedChange={value => update({ includeApprovedNotes: value === true })} />
            <Label htmlFor="series-include-notes">Include approved notes in preparation briefs</Label>
          </div>
          <DialogFooter>
            <Button type="button" variant="outline" disabled={busy} onClick={close}>Cancel</Button>
            <Button type="submit" disabled={busy}>{busy ? "Saving…" : "Save series preferences"}</Button>
          </DialogFooter>
        </form>
      </>}
    </DialogContent>
  </Dialog>;
}
