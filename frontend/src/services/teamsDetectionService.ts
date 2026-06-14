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
