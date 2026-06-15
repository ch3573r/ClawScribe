/**
 * Teams Detection Service
 *
 * Read-only wrappers for the Teams meeting detector. These calls only observe
 * process/window evidence; they never start or stop recording.
 */

import { invoke } from '@tauri-apps/api/core';

export type TeamsDetectionState =
  | 'unsupported'
  | 'disabled'
  | 'notDetected'
  | 'possible'
  | 'detected';

export type TeamsDetectionAction =
  | 'idle'
  | 'promptToRecord'
  | 'unsupported'
  | 'disabled';

export interface TeamsDetectionConfig {
  enabled: boolean;
  confidenceThreshold: number;
  requireMeetingTitleSignal: boolean;
  maxWindowTitleSamples: number;
}

export interface TeamsDetectionSignal {
  detector: string;
  matched: boolean;
  confidence: number;
  detail: string;
}

export interface TeamsDetectionDiagnostics {
  processCount: number;
  teamsProcessCount: number;
  browserProcessCount: number;
  relevantWindowCount: number;
  meetingTitleCount: number;
  browserMeetingTitleCount: number;
  foregroundMeetingTitleCount: number;
  windowSampleLimit: number;
  titleSignalRequired: boolean;
  titleSignalSatisfied: boolean;
  confidenceCappedByTitleRequirement: boolean;
}

export interface TeamsDetectionRecordingSafety {
  mode: string;
  automaticRecordingAllowed: boolean;
  promptRequired: boolean;
  detail: string;
}

export interface TeamsDetectionCandidate {
  source: string;
  processId: number | null;
  processName: string | null;
  windowTitle: string | null;
  isForeground: boolean;
  isMinimized: boolean;
  confidence: number;
}

export interface TeamsDetectionStatus {
  supported: boolean;
  enabled: boolean;
  platform: string;
  status: TeamsDetectionState;
  detected: boolean;
  confidence: number;
  threshold: number;
  requireMeetingTitleSignal: boolean;
  reason: string;
  signals: TeamsDetectionSignal[];
  candidates: TeamsDetectionCandidate[];
  diagnostics: TeamsDetectionDiagnostics;
  recordingSafety: TeamsDetectionRecordingSafety;
  nextRecommendedAction: TeamsDetectionAction;
}

export class TeamsDetectionService {
  async getConfig(): Promise<TeamsDetectionConfig> {
    return invoke<TeamsDetectionConfig>('get_teams_detection_config');
  }

  async getStatus(config?: TeamsDetectionConfig): Promise<TeamsDetectionStatus> {
    return invoke<TeamsDetectionStatus>('get_teams_detection_status', {
      config: config ?? null,
    });
  }
}

export const teamsDetectionService = new TeamsDetectionService();

export interface TeamsDetectionDebugBridge {
  getConfig: () => Promise<TeamsDetectionConfig>;
  getStatus: (config?: TeamsDetectionConfig) => Promise<TeamsDetectionStatus>;
  printStatus: (config?: TeamsDetectionConfig) => Promise<TeamsDetectionStatus>;
}

declare global {
  interface Window {
    __clawscribeTeamsDetection?: TeamsDetectionDebugBridge;
  }
}

export function installTeamsDetectionDebugBridge(): void {
  if (typeof window === 'undefined' || process.env.NODE_ENV === 'production') {
    return;
  }

  window.__clawscribeTeamsDetection = {
    getConfig: () => teamsDetectionService.getConfig(),
    getStatus: (config?: TeamsDetectionConfig) => teamsDetectionService.getStatus(config),
    printStatus: async (config?: TeamsDetectionConfig) => {
      const status = await teamsDetectionService.getStatus(config);
      console.log('[ClawScribe] Teams detection status', status);
      console.table(status.signals);
      console.table([status.diagnostics]);
      console.table([status.recordingSafety]);
      return status;
    },
  };
}
