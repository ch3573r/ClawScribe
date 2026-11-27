/**
 * Recording Service
 *
 * Handles all recording lifecycle Tauri backend calls and events.
 * Pure 1-to-1 wrapper - no error handling changes, exact same behavior as direct invoke/listen calls.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import type { MeetingCalendarLink } from '@/lib/meetingCalendar';
import type { MissingRecordingDevice, RecordingHealth } from '@/lib/recording-health';

export interface RecordingState {
  session_id?: string | null;
  recording_generation?: string | null;
  recording_mode?: 'live' | 'audio_only';
  is_recording: boolean;
  is_stopping?: boolean;
  is_paused: boolean;
  is_active: boolean;
  recording_duration: number | null;
  active_duration: number | null;
}

interface RecordingStartedPayload {
  session_id?: string | null;
  recording_generation?: string | null;
  recording_mode?: 'live' | 'audio_only';
  recording_id?: string | null;
  folder_path?: string | null;
  meeting_name?: string | null;
}

export interface RecordingStoppedPayload {
  session_id?: string | null;
  recording_generation?: string | null;
  recording_mode?: 'live' | 'audio_only';
  audio_save_failed?: boolean;
  audio_saved?: boolean;
  capture_incomplete?: boolean;
  recording_files_incomplete?: boolean;
  transcription_incomplete?: boolean;
  message: string;
  folder_path?: string;
  meeting_name?: string;
  meeting_id?: string | null;
  recording_id?: string | null;
  transcript_count?: number;
  library_save_failed?: boolean;
}

/**
 * Recording Service
 * Singleton service for managing recording lifecycle operations
 */
export class RecordingService {
  /**
   * Get comprehensive recording state (includes durations)
   * @returns Promise with full recording state
   */
  async getRecordingState(): Promise<RecordingState> {
    return invoke<RecordingState>('get_recording_state');
  }

  /**
   * Get current meeting name
   * @returns Promise<string | null>
   */
  async getRecordingMeetingName(): Promise<string | null> {
    return invoke<string | null>('get_recording_meeting_name');
  }

  /**
   * Start recording (no device configuration)
   * @returns Promise<void>
   */
  async startRecording(): Promise<void> {
    return invoke('start_recording');
  }

  /**
   * Start recording with device configuration and meeting name
   * @param micDeviceName - Microphone device name (null for default)
   * @param systemDeviceName - System audio device name (null for default when available)
   * @param meetingName - Meeting name/title
   * @returns Native recording session ID
   */
  async startRecordingWithDevices(
    micDeviceName: string | null,
    systemDeviceName: string | null,
    meetingName: string,
    calendarLink?: MeetingCalendarLink | null,
    requestId?: string,
    summaryLanguage?: string | null,
    customTitle?: string,
  ): Promise<string> {
    return invoke('start_recording_with_devices_and_meeting', {
      micDeviceName,
      systemDeviceName,
      meetingName,
      useDefaultMicrophone: true,
      calendarLink,
      requestId,
      summaryLanguage,
      customTitle,
    });
  }

  async stopDetectedCall(sessionId: string): Promise<boolean> {
    return invoke('stop_detected_call_recording', { sessionId });
  }

  /**
   * Stop the active recording and save it to the library
   * @returns Whether this call stopped a recording
   */
  async stopRecording(): Promise<boolean> {
    return invoke<boolean>('stop_recording');
  }

  /**
   * Pause active recording
   * @returns Promise<void>
   */
  async pauseRecording(): Promise<void> {
    return invoke('pause_recording');
  }

  /**
   * Resume paused recording
   * @returns Promise<void>
   */
  async resumeRecording(): Promise<void> {
    return invoke('resume_recording');
  }

  async getRecordingHealth(sessionId: string): Promise<RecordingHealth | null> {
    return invoke('get_recording_health', { sessionId });
  }

  async attemptDeviceReconnect(deviceName: string, deviceType: MissingRecordingDevice['device_type'], sessionId: string) {
    return invoke<{ reconnected: boolean }>('attempt_device_reconnect', { deviceName, deviceType, sessionId });
  }

  // Event Listeners

  /**
   * Listen for recording-started event
   * @param callback - Function to call when recording starts
   * @returns Promise that resolves to unlisten function
   */
  async onRecordingStarted(callback: (mode: 'live' | 'audio_only', sessionId?: string | null, generation?: string | null, metadata?: RecordingStartedPayload) => void): Promise<UnlistenFn> {
    return listen<RecordingStartedPayload>('recording-started', ({ payload }) =>
      callback(payload.recording_mode ?? 'live', payload.session_id, payload.recording_generation, payload));
  }

  /**
   * Listen for recording-stopped event (with metadata)
   * @param callback - Function to call when recording stops
   * @returns Promise that resolves to unlisten function
   */
  async onRecordingStopped(callback: (payload: RecordingStoppedPayload) => void): Promise<UnlistenFn> {
    return listen<RecordingStoppedPayload>('recording-stopped', (event) => {
      callback(event.payload);
    });
  }

  /**
   * Listen for recording-paused event
   * @param callback - Function to call when recording is paused
   * @returns Promise that resolves to unlisten function
   */
  async onRecordingPaused(callback: () => void): Promise<UnlistenFn> {
    return listen('recording-paused', callback);
  }

  /**
   * Listen for recording-resumed event
   * @param callback - Function to call when recording resumes
   * @returns Promise that resolves to unlisten function
   */
  async onRecordingResumed(callback: () => void): Promise<UnlistenFn> {
    return listen('recording-resumed', callback);
  }

  /**
   * Listen for chunk-drop-warning event (audio buffer overflow)
   * @param callback - Function to call when chunks are dropped
   * @returns Promise that resolves to unlisten function
   */
  async onChunkDropWarning(callback: (warning: string) => void): Promise<UnlistenFn> {
    return listen<string>('chunk-drop-warning', (event) => {
      callback(event.payload);
    });
  }
}

// Export singleton instance
export const recordingService = new RecordingService();
