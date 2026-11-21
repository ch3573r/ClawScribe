// Telemetry has been removed from ClawScribe.
//
// This module is intentionally a no-op shim. It preserves the historical
// `Analytics` API surface so the many existing call sites keep compiling, but
// every method does nothing and no data ever leaves the device. There are no
// network calls, no PostHog, and no persistent identifiers.
//
// Every method accepts variadic args so any existing call signature is
// accepted without a type error.

export interface AnalyticsProperties {
  [key: string]: string | number | boolean | null | undefined;
}

export interface DeviceInfo {
  platform?: string;
  os_version?: string;
  app_version?: string;
  architecture?: string;
  [key: string]: string | number | boolean | null | undefined;
}

export interface UserSession {
  sessionId?: string;
  startedAt?: number;
  [key: string]: unknown;
}

type CompatibilityMethod<T> = (...args: unknown[]) => T;
const ignoreEvent: CompatibilityMethod<Promise<void>> = async () => {};
const disabled: CompatibilityMethod<Promise<boolean>> = async () => false;
const emptyString: CompatibilityMethod<Promise<string>> = async () => "";

/** No-op analytics. All methods are inert. */
export class Analytics {
  // ── lifecycle ───────────────────────────────────────────────────────────
  static init = ignoreEvent;
  static doInit = ignoreEvent;
  static cleanup = ignoreEvent;
  static disable = ignoreEvent;
  static reset = ignoreEvent;
  static waitForInitialization = disabled;
  static isEnabled = disabled;

  // ── identity / session ──────────────────────────────────────────────────
  static identify = ignoreEvent;
  static getCurrentUserId: CompatibilityMethod<string | null> = () => null;
  static getPersistentUserId = emptyString;
  static startSession: CompatibilityMethod<Promise<string | null>> = async () => null;
  static endSession = ignoreEvent;
  static isSessionActive = disabled;

  // ── device info ─────────────────────────────────────────────────────────
  static getPlatform = emptyString;
  static getOSVersion = emptyString;
  static getDeviceInfo: CompatibilityMethod<Promise<DeviceInfo>> = async () => ({});

  // ── usage bookkeeping ───────────────────────────────────────────────────
  static calculateDaysSince: CompatibilityMethod<Promise<number | null>> = async () => null;
  static getMeetingsCountToday: CompatibilityMethod<Promise<number>> = async () => 0;
  static updateMeetingCount = ignoreEvent;
  static hasUsedFeatureBefore = disabled;
  static markFeatureUsed = ignoreEvent;
  static checkAndTrackFirstLaunch = ignoreEvent;
  static checkAndTrackDailyUsage = ignoreEvent;
  static trackDailyActiveUser = ignoreEvent;
  static trackUserFirstLaunch = ignoreEvent;

  // ── generic + named tracking (all inert) ────────────────────────────────
  static track = ignoreEvent;
  static trackEvent = ignoreEvent;
  static trackButtonClick = ignoreEvent;
  static trackPageView = ignoreEvent;
  static trackFeatureUsed = ignoreEvent;
  static trackFeatureUsedEnhanced = ignoreEvent;
  static trackError = ignoreEvent;
  static trackCopy = ignoreEvent;
  static trackCustomPromptUsed = ignoreEvent;
  static trackAppStarted = ignoreEvent;
  static trackBackendConnection = ignoreEvent;
  static trackModelChanged = ignoreEvent;
  static trackSettingsChanged = ignoreEvent;
  static trackMeetingStarted = ignoreEvent;
  static trackMeetingCompleted = ignoreEvent;
  static trackMeetingDeleted = ignoreEvent;
  static trackRecordingStarted = ignoreEvent;
  static trackRecordingStopped = ignoreEvent;
  static trackTranscriptionSuccess = ignoreEvent;
  static trackTranscriptionError = ignoreEvent;
  static trackSummaryGenerationStarted = ignoreEvent;
  static trackSummaryGenerationCompleted = ignoreEvent;
  static trackSummaryRegenerated = ignoreEvent;
  static trackSessionStarted = ignoreEvent;
  static trackSessionEnded = ignoreEvent;
}

export default Analytics;
