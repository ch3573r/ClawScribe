export type FollowUpDestination = "planner" | "todo" | "email";
export const SERIES_REVIEW_CHANGED = "clawscribe-series-review-changed";

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
