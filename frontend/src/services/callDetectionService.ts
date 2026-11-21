/**
 * Call detection and background recording requests.
 *
 * The core decides when a detected call starts or stops a recording, and it
 * asks for a recording to start from the tray, a shortcut or the command line.
 * The window only carries out the start (it knows the chosen audio devices and
 * the calendar title) and answers. These are the thin wrappers for that.
 */

import { invoke } from '@tauri-apps/api/core';
import type { CallProvider, CallSettings } from '@/lib/autoRecord';

export type StartRequestSource = 'tray' | 'shortcut' | 'cli' | 'call' | 'calendar_call' | 'after_save';

export interface StartRequest {
  requestId: string;
  source: StartRequestSource;
  provider: CallProvider | null;
}

export interface CallPrompt {
  key: string;
  provider: CallProvider;
  /** An automatic start failed and the user may try again. */
  retry: boolean;
  eventTitle?: string | null;
}

export interface CallNotice {
  kind: 'started' | 'stopped' | 'stop-failed';
  provider: CallProvider;
  label: string;
}

/**
 * The window may decline before the native command claims the request.
 */
export type StartStatus = 'busy' | 'failed' | 'refused';

/** The settings the core holds, or null when it has none yet. */
export function getCoreCallSettings(): Promise<CallSettings | null> {
  return invoke('get_call_detection_settings');
}

export function setCoreCallSettings(settings: CallSettings): Promise<void> {
  return invoke('set_call_detection_settings', { settings });
}

export function getPendingStartRequest(): Promise<StartRequest | null> {
  return invoke('get_pending_start_request');
}

export function finishStartRequest(
  requestId: string,
  status: StartStatus,
): Promise<boolean> {
  return invoke('finish_recording_start', { requestId, status });
}

export function getPendingCallPrompt(): Promise<CallPrompt | null> {
  return invoke('get_pending_call_prompt');
}

export function confirmCallPrompt(key: string): Promise<void> {
  return invoke('confirm_call_detection_prompt', { key });
}
