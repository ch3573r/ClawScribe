import { invoke } from "@tauri-apps/api/core";

export interface MicrosoftConnectionInfo {
  state:
    | "not_connected"
    | "connecting"
    | "connected"
    | "consent_required"
    | "tenant_blocked"
    | "access_denied"
    | "expired";
  userDisplayName: string | null;
  userEmail: string | null;
  grantedScopes?: string | null;
}

export interface ExportItemResult {
  dedupeKey: string;
  localId: string;
  status: string;
  resourceId: string | null;
  webUrl: string | null;
  code: string | null;
  graphCalled: boolean;
}

export interface ExportReport {
  overall: string;
  connectionState: string | null;
  items: ExportItemResult[];
}

export interface NotebookInfo {
  id: string;
  displayName: string;
}
export interface SectionInfo {
  id: string;
  displayName: string;
}
export interface PlanInfo {
  id: string;
  title: string;
}
export interface BucketInfo {
  id: string;
  name: string;
}
export interface CalendarAttendee {
  name: string | null;
  email: string | null;
}
export interface CalendarEvent {
  id: string;
  subject: string | null;
  /** ISO-8601 UTC, e.g. "2026-06-19T21:00:00Z". */
  start: string | null;
  end: string | null;
  isOnlineMeeting: boolean;
  joinUrl: string | null;
  organizer: CalendarAttendee | null;
  attendees: CalendarAttendee[];
}

export interface PlannerTaskPreview {
  localId: string;
  title: string;
  details: string;
  owner: string | null;
  dueDate: string | null;
}

export interface PlannerTaskInput {
  // Stable id from the preview (PlannerTaskPreview.localId). Sent back so the
  // backend dedupe key survives reordering/deselection and re-exports don't
  // create duplicate tasks.
  localId: string;
  title: string;
  owner: string | null;
  dueDate: string | null;
  bucketId: string;
  details?: string | null;
}

export interface PolishedPlannerTask {
  title: string;
  details: string;
}

export const microsoftExportService = {
  async signIn(): Promise<void> {
    // Opens the system browser for interactive sign-in; completion arrives via
    // the `microsoft-auth-complete` event.
    return invoke<void>("microsoft_sign_in");
  },

  async signOut(): Promise<void> {
    return invoke<void>("microsoft_sign_out");
  },

  async connectionStatus(): Promise<MicrosoftConnectionInfo> {
    return invoke<MicrosoftConnectionInfo>("microsoft_connection_status");
  },

  async exportToOneNote(
    meetingId: string,
    meetingTitle: string,
    summaryJson: string,
    sectionId: string,
  ): Promise<ExportReport> {
    return invoke<ExportReport>("export_to_onenote", {
      meetingId,
      meetingTitle,
      summaryJson,
      sectionId,
    });
  },

  async exportToPlanner(
    meetingId: string,
    meetingTitle: string,
    summaryJson: string,
    planId: string,
    bucketId: string,
  ): Promise<ExportReport> {
    return invoke<ExportReport>("export_to_planner", {
      meetingId,
      meetingTitle,
      summaryJson,
      planId,
      bucketId,
    });
  },

  async summaryHasActionItems(markdown: string): Promise<boolean> {
    return invoke<boolean>("summary_has_action_items", { markdown });
  },

  async exportMeetingMarkdownToOneNote(
    meetingId: string,
    meetingTitle: string,
    markdown: string,
    sectionId: string,
  ): Promise<ExportReport> {
    return invoke<ExportReport>("export_meeting_markdown_to_onenote", {
      meetingId,
      meetingTitle,
      markdown,
      sectionId,
    });
  },

  async exportMeetingToOneNoteSection(
    meetingId: string,
    meetingTitle: string,
    markdown: string,
    notebookId: string,
    sectionName: string,
  ): Promise<ExportReport> {
    return invoke<ExportReport>("export_meeting_to_onenote_section", {
      meetingId,
      meetingTitle,
      markdown,
      notebookId,
      sectionName,
    });
  },

  async exportMeetingMarkdownToPlanner(
    meetingId: string,
    meetingTitle: string,
    markdown: string,
    planId: string,
    bucketId: string,
  ): Promise<ExportReport> {
    return invoke<ExportReport>("export_meeting_markdown_to_planner", {
      meetingId,
      meetingTitle,
      markdown,
      planId,
      bucketId,
    });
  },

  async listNotebooks(): Promise<NotebookInfo[]> {
    return invoke<NotebookInfo[]>("list_onenote_notebooks");
  },

  async listSections(notebookId: string): Promise<SectionInfo[]> {
    return invoke<SectionInfo[]>("list_onenote_sections", { notebookId });
  },

  async listPlans(): Promise<PlanInfo[]> {
    return invoke<PlanInfo[]>("list_planner_plans");
  },

  async listBuckets(planId: string): Promise<BucketInfo[]> {
    return invoke<BucketInfo[]>("list_planner_buckets", { planId });
  },

  async createNotebook(displayName: string): Promise<NotebookInfo> {
    return invoke<NotebookInfo>("create_onenote_notebook", { displayName });
  },

  async listCalendarEvents(
    startIso: string,
    endIso: string,
  ): Promise<CalendarEvent[]> {
    return invoke<CalendarEvent[]>("list_calendar_events", { startIso, endIso });
  },

  async currentOrNextMeeting(): Promise<CalendarEvent | null> {
    return invoke<CalendarEvent | null>("current_or_next_meeting");
  },

  async previewPlannerTasks(
    meetingId: string,
    meetingTitle: string,
    markdown: string,
  ): Promise<PlannerTaskPreview[]> {
    return invoke<PlannerTaskPreview[]>("preview_planner_tasks", {
      meetingId,
      meetingTitle,
      markdown,
    });
  },

  async exportSelectedPlannerTasks(
    meetingId: string,
    meetingTitle: string,
    planId: string,
    tasks: PlannerTaskInput[],
  ): Promise<ExportReport> {
    return invoke<ExportReport>("export_selected_planner_tasks", {
      meetingId,
      meetingTitle,
      planId,
      tasks,
    });
  },

  async polishPlannerTasks(
    model: string,
    modelName: string,
    tasks: Array<{ title: string; owner: string | null; dueDate: string | null }>,
  ): Promise<PolishedPlannerTask[]> {
    return invoke<PolishedPlannerTask[]>("polish_planner_tasks", {
      model,
      modelName,
      tasks,
    });
  },

  async createBucket(planId: string, name: string): Promise<BucketInfo> {
    return invoke<BucketInfo>("create_planner_bucket", { planId, name });
  },
};
