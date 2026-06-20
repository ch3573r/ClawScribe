"use client";

import { invoke } from "@tauri-apps/api/core";

export interface ConfluenceConnectionStatus {
  tokenConfigured: boolean;
  reachable: boolean;
  userDisplayName: string | null;
  message: string;
}

export interface ConfluenceExportResponse {
  pageId: string;
  title: string;
  webUrl: string | null;
}

export const confluenceExportService = {
  savePat(pat: string): Promise<void> {
    return invoke("confluence_save_pat", { pat });
  },

  clearPat(): Promise<void> {
    return invoke("confluence_clear_pat");
  },

  connectionStatus(baseUrl: string): Promise<ConfluenceConnectionStatus> {
    return invoke("confluence_connection_status", { baseUrl });
  },

  exportPage(args: {
    baseUrl: string;
    spaceKey: string;
    parentId?: string | null;
    title: string;
    bodyStorage: string;
  }): Promise<ConfluenceExportResponse> {
    return invoke("confluence_export_page", args);
  },
};
