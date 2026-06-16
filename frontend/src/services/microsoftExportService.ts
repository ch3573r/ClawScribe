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
};
