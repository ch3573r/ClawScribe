export type FollowUpDestination = "planner" | "todo" | "email";
export const SERIES_REVIEW_CHANGED = "clawscribe-series-review-changed";
export const REVIEWED_ACTIONS_CHANGED = "reviewed-actions-changed";
export const MICROSOFT_EXPORT_SETTINGS = "/settings?tab=ms365";

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
