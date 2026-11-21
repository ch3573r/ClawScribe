import type { SummaryDataResponse } from '@/types';

export interface SummaryJobSnapshot {
  status: string;
  meetingName?: string | null;
  data?: string | SummaryDataResponse | null;
  error?: string | null;
  partial_notes?: string | null;
  progress?: { completed: number; total: number; level: number } | null;
}

export function isActiveSummary(status?: string): boolean {
  return status !== undefined && ['pending', 'processing', 'summarizing', 'regenerating'].includes(status);
}

export function isTerminalSummary(status: string): boolean {
  return ['completed', 'failed', 'cancelled', 'skipped', 'idle', 'error'].includes(status);
}
