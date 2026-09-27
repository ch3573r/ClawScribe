"use client";

import { invoke } from "@tauri-apps/api/core";

export interface ConfluenceConnectionStatus {
  baseUrl?: string | null;
  destinationProblem?: string | null;
  tokenConfigured: boolean;
  allowUnencrypted: boolean;
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
  savePat(pat: string, baseUrl: string, allowUnencrypted = false): Promise<void> {
    return invoke("confluence_save_pat", { pat, baseUrl, allowUnencrypted });
  },

  settingsStatus(baseUrl: string): Promise<ConfluenceConnectionStatus> {
    return invoke("confluence_settings_status", { baseUrl });
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
