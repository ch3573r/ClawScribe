// Persisted Microsoft export destinations.
//
// The OneNote notebook/section and Planner plan/bucket chosen in
// Settings → Add-ons are saved here so the per-meeting export buttons in the
// summary view know where to send. These are non-sensitive Graph IDs, stored
// in localStorage.

const STORAGE_KEY = "clawscribe.exportDestinations.v1";

export type ConfluenceExportMode = "draft" | "rest";

export interface ExportDestinations {
  notebookId?: string;
  notebookName?: string;
  sectionId?: string;
  sectionName?: string;
  planId?: string;
  planName?: string;
  bucketId?: string;
  bucketName?: string;
  /** AI-polish Planner task titles & notes before export (default off). */
  plannerAiPolish?: boolean;
  /** Confluence export mode. Draft is clipboard/browser only; REST uses a saved PAT. */
  confluenceMode?: ConfluenceExportMode;
  /** Browser URL for creating a Confluence page draft. No REST call is made. */
  confluenceCreateUrl?: string;
  /** Open the configured Confluence page URL after copying the draft. */
  confluenceOpenAfterCopy?: boolean;
  /** Self-hosted Confluence base URL for REST export. Non-sensitive. */
  confluenceBaseUrl?: string;
  /** Target Confluence space key for REST export. Non-sensitive. */
  confluenceSpaceKey?: string;
  /** Optional parent page id for REST export. Non-sensitive. */
  confluenceParentId?: string;
}

export function getExportDestinations(): ExportDestinations {
  if (typeof window === "undefined") return {};
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    return raw ? (JSON.parse(raw) as ExportDestinations) : {};
  } catch {
    return {};
  }
}

export function setExportDestinations(patch: Partial<ExportDestinations>): ExportDestinations {
  if (typeof window === "undefined") return {};
  const next = { ...getExportDestinations(), ...patch };
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch {
    // ignore quota / disabled storage
  }
  return next;
}

export function hasOneNoteDestination(d: ExportDestinations = getExportDestinations()): boolean {
  // A notebook is enough: the section is created per-export (a dated section),
  // so we never need a pre-selected section id.
  return !!d.notebookId;
}

export function hasPlannerDestination(d: ExportDestinations = getExportDestinations()): boolean {
  return !!d.planId && !!d.bucketId;
}

export function hasConfluenceDraftTarget(d: ExportDestinations = getExportDestinations()): boolean {
  return !!d.confluenceCreateUrl?.trim();
}
