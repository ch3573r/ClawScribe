import { markdownToPlainText } from "./clipboard-format";
import { calendarLinkFromEvent, type MeetingCalendarLink } from "./meetingCalendar";
import type { UpcomingEntry } from "@/hooks/useUpcomingEvents";

export type FollowUpDestination = "planner" | "todo" | "email";
export const SERIES_REVIEW_CHANGED = "clawscribe-series-review-changed";
export const REVIEWED_ACTIONS_CHANGED = "reviewed-actions-changed";
export const MICROSOFT_EXPORT_SETTINGS = "/settings?tab=ms365";
export const SERIES_NOTES_LIMIT = 2000;

export function upcomingSeriesLink({ event, sourceId }: UpcomingEntry): MeetingCalendarLink | null {
  return event.seriesId && event.start
    ? calendarLinkFromEvent({ ...event, sourceId, attendees: [] })
    : null;
}

interface DraftSummary {
  markdown?: string;
  structured_meeting_output?: {
    executive_summary?: string;
    decisions?: { decision?: string }[];
  };
}

export function draftSeriesNotes(data: DraftSummary | null, current: string): string | null {
  const structured = data?.structured_meeting_output;
  const plain = (text: unknown) => typeof text === "string" ? markdownToPlainText(text).trim() : "";
  const decisions = Array.isArray(structured?.decisions)
    ? [...new Set(structured.decisions.map(item => plain(item?.decision)).filter(Boolean))]
    : [];
  const summary = plain(structured?.executive_summary) || plain(data?.markdown);
  const sections = [
    decisions.length ? `Decisions\n${decisions.map(decision => `- ${decision}`).join("\n")}` : "",
    summary ? `Summary\n${summary}` : "",
  ].filter(Boolean);
  const draft = sections.join("\n\n");
  const prefix = current.trimEnd() ? `${current.trimEnd()}\n\n` : "";
  const available = SERIES_NOTES_LIMIT - prefix.length;
  if (!draft || current.includes(draft) || available < (decisions.length ? 13 : 9)) return null;
  const excerpt = draft.length > available
    ? `${draft.slice(0, available - 1).replace(/[\uD800-\uDBFF]$/u, "").trimEnd()}…`
    : draft;
  return prefix + excerpt;
}

export interface SeriesPreferences {
  language: string | null;
  includeApprovedNotes: boolean;
  destination: FollowUpDestination | null;
}

export interface OpenAction {
  id: string;
  title: string;
  owner: string | null;
  dueDate: string | null;
  sourceKey: string | null;
  done: boolean;
}

export interface OpenActionRow {
  meetingId: string;
  meetingTitle: string;
  createdAt: string;
  action: OpenAction;
}

export interface ActionExportGroup {
  meetingId: string;
  meetingTitle: string;
  actionIds: string[];
}

export interface MeetingActionsOverview {
  openCount: number;
  hasSuggestions: boolean;
}

export function meetingActionsLabel(overview: MeetingActionsOverview | null) {
  if (!overview) return "Actions";
  return `Actions (${overview.openCount})${overview.hasSuggestions ? ", new summary suggestions" : ""}`;
}

export function actionRowKey(meetingId: string, actionId: string) {
  return JSON.stringify([meetingId, actionId]);
}

export function groupSelectedActions(rows: readonly OpenActionRow[], selected: ReadonlySet<string>) {
  const groups = new Map<string, ActionExportGroup>();
  for (const { meetingId, meetingTitle, action } of rows) {
    if (action.done || !selected.has(actionRowKey(meetingId, action.id))) continue;
    let group = groups.get(meetingId);
    if (!group) {
      group = { meetingId, meetingTitle, actionIds: [] };
      groups.set(meetingId, group);
    }
    group.actionIds.push(action.id);
  }
  return [...groups.values()];
}

export interface SeriesOccurrence {
  meetingId: string;
  title: string;
  startsAt: string;
  approvedNotes: string | null;
  actions: OpenAction[];
}

export function meetingActionUrl(meetingId: string, actionId: string, fromArchive = true) {
  const query = new URLSearchParams({ id: meetingId, archiveAction: "actions", actionId });
  if (fromArchive) query.set("from", "archive");
  return `/meeting-details?${query}`;
}
