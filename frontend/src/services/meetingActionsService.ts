import { invoke } from "@tauri-apps/api/core";
import type { MeetingActionsOverview, OpenAction } from "@/lib/meeting-follow-through";

type FieldEdit<T> = { from: T; to: T };

export interface ReviewedActionEdit {
  id: string;
  title?: FieldEdit<string>;
  details?: FieldEdit<string | null>;
  owner?: FieldEdit<string | null>;
  dueDate?: FieldEdit<string | null>;
  done?: FieldEdit<boolean>;
}

export interface SavedAction extends OpenAction {
  details?: string;
  legacyAction?: { id: string; titleHash: string } | null;
}

export interface SummaryActionEdit {
  localId: string;
  originalTitleHash: string;
  title?: string;
  details?: string;
}

export function getMeetingActionsOverview(meetingId: string, markdown: string) {
  return invoke<MeetingActionsOverview>("get_meeting_actions_overview", { meetingId, markdown });
}

export function editReviewedActions(meetingId: string, edits: ReviewedActionEdit[]) {
  return invoke<SavedAction[]>("edit_reviewed_meeting_actions", { meetingId, edits });
}

export function savePreviewedSummaryActions(meetingId: string, markdown: string, edits: SummaryActionEdit[]) {
  return invoke<SavedAction[]>("save_previewed_summary_actions", { meetingId, markdown, edits });
}
