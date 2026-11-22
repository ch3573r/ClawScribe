"use client";

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import Link from "next/link";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { PageSection } from "@/components/ui/page-section";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { formatArchiveDate } from "@/lib/archive-review";
import { meetingActionUrl, type OpenAction } from "@/lib/meeting-follow-through";

interface ActionPage {
  rows: { meetingId: string; meetingTitle: string; createdAt: string; action: OpenAction }[];
  series: { id: string; label: string; createdAt: string }[];
  hasMore: boolean;
  skippedMeetings: number;
}

const initialFilters = { owner: "", dueFrom: "", dueTo: "", series: "" };
let previousFilters = initialFilters;

export function OpenMeetingActions() {
  const [filters, setFilters] = useState(previousFilters);
  const [result, setResult] = useState<ActionPage | null>(null);
  const [page, setPage] = useState(0);
  const [revision, setRevision] = useState(0);
  const [error, setError] = useState("");
  function filter(key: keyof typeof filters, value: string) {
    setFilters(current => previousFilters = { ...current, [key]: value });
    setPage(0);
  }

  useEffect(() => {
    let active = true;
    setError("");
    setResult(null);
    const timer = window.setTimeout(() => {
      const args = Object.fromEntries(Object.entries(filters).map(([key, value]) => [key, value || null]));
      void invoke<ActionPage>("list_open_reviewed_actions", { filters: args, page }).then(rows => {
        if (active) setResult(rows);
      }).catch(failure => {
        if (active) setError(typeof failure === "string" ? failure : "Could not load open actions.");
      });
    }, 180);
    return () => { active = false; window.clearTimeout(timer); };
  }, [filters, page, revision]);

  return <PageSection title="Open actions" description="Review and complete follow-ups from saved meetings." actions={
    <Button variant="outline" size="sm" onClick={() => setRevision(value => value + 1)}>Refresh</Button>
  }>
    <div className="grid gap-3 border-b border-border p-5 sm:grid-cols-4">
      <div className="min-w-0 space-y-1">
        <Label htmlFor="follow-up-owner">Owner</Label>
        <Input id="follow-up-owner" value={filters.owner} maxLength={160} placeholder="All owners" onChange={event => filter("owner", event.target.value)} />
      </div>
      <div className="space-y-1">
        <Label htmlFor="follow-up-from">Due from</Label>
        <Input id="follow-up-from" type="date" value={filters.dueFrom} onChange={event => filter("dueFrom", event.target.value)} />
      </div>
      <div className="space-y-1">
        <Label htmlFor="follow-up-to">Due through</Label>
        <Input id="follow-up-to" type="date" value={filters.dueTo} onChange={event => filter("dueTo", event.target.value)} />
      </div>
      <div className="min-w-0 space-y-1">
        <Label htmlFor="follow-up-series">Recurring series</Label>
        <Select value={filters.series || "__all__"} onValueChange={value => filter("series", value === "__all__" ? "" : value)}>
          <SelectTrigger id="follow-up-series"><SelectValue placeholder="All meetings" /></SelectTrigger>
          <SelectContent>
            <SelectItem value="__all__">All meetings</SelectItem>
            {result?.series.map(series => <SelectItem key={series.id} value={series.id}>{series.label} · {formatArchiveDate(series.createdAt)}</SelectItem>)}
          </SelectContent>
        </Select>
      </div>
    </div>
    <div className="p-5">
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      {!result && !error && <p role="status" className="text-sm text-muted-foreground">Loading open actions…</p>}
      {!result?.rows.length && result && <p className="text-sm text-muted-foreground">No open actions match these filters.</p>}
      {!!result?.skippedMeetings && <p role="status" className="text-sm text-muted-foreground">Some saved actions could not be read. Open their meetings to review them.</p>}
      {!!result?.rows.length && <ul className="divide-y divide-border">
        {result.rows.map(({ action, meetingId, meetingTitle, createdAt }) => <li key={`${meetingId}:${action.id}`} className="flex items-start justify-between gap-3 py-3">
          <div className="min-w-0">
            <p className="break-words font-medium">{action.title}</p>
            <p className="break-words text-sm text-muted-foreground">{meetingTitle} · {formatArchiveDate(createdAt)}</p>
            <p className="break-words text-sm text-muted-foreground">{action.owner || "Unassigned"}{action.dueDate ? ` · Due ${action.dueDate}` : ""}</p>
          </div>
          <Button variant="outline" size="sm" asChild>
            <Link href={meetingActionUrl(meetingId, action.id)}>Review action</Link>
          </Button>
        </li>)}
      </ul>}
      {(page > 0 || result?.hasMore) && <div className="mt-4 flex justify-end gap-2">
        <Button variant="outline" size="sm" disabled={page === 0} onClick={() => setPage(value => value - 1)}>Previous</Button>
        <Button variant="outline" size="sm" disabled={!result?.hasMore} onClick={() => setPage(value => value + 1)}>Next</Button>
      </div>}
    </div>
  </PageSection>;
}
