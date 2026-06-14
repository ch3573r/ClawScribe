"use client"

import { useCallback, useEffect, useState, useRef } from "react"
import { Switch } from "./ui/switch"
import { FolderOpen, RefreshCw, ServerCog } from "lucide-react"
import { invoke } from "@tauri-apps/api/core"
import Analytics from "@/lib/analytics"
import AnalyticsConsentSwitch from "./AnalyticsConsentSwitch"
import { useConfig, NotificationSettings } from "@/contexts/ConfigContext"

type OpenClawSubmissionStatus = {
  state: string
  updated_at: string
  status_code?: number | null
  message: string
  endpoint?: string | null
  source?: string | null
  idempotency_key?: string | null
}

type OpenClawConfigStatus = {
  enabled: boolean
  configured: boolean
  ready: boolean
  bearer_token_configured: boolean
  endpoint: string
  source: string
  status_message: string
  config_path: string
  last_status_path: string
  include_audio_path: boolean
  last_submission?: OpenClawSubmissionStatus | null
}

export function PreferenceSettings() {
  const {
    notificationSettings,
    storageLocations,
    isLoadingPreferences,
    loadPreferences,
    updateNotificationSettings
  } = useConfig();

  const [notificationsEnabled, setNotificationsEnabled] = useState<boolean | null>(null);
  const [isInitialLoad, setIsInitialLoad] = useState(true);
  const [previousNotificationsEnabled, setPreviousNotificationsEnabled] = useState<boolean | null>(null);
  const [openClawStatus, setOpenClawStatus] = useState<OpenClawConfigStatus | null>(null);
  const [openClawStatusError, setOpenClawStatusError] = useState<string | null>(null);
  const [isOpenClawStatusLoading, setIsOpenClawStatusLoading] = useState(false);
  const hasTrackedViewRef = useRef(false);

  const loadOpenClawStatus = useCallback(async () => {
    setIsOpenClawStatusLoading(true);
    setOpenClawStatusError(null);

    try {
      const status = await invoke('get_openclaw_config_status') as OpenClawConfigStatus;
      setOpenClawStatus(status);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setOpenClawStatusError(message);
    } finally {
      setIsOpenClawStatusLoading(false);
    }
  }, []);

  // Lazy load preferences on mount (only loads if not already cached)
  useEffect(() => {
    loadPreferences();
    void loadOpenClawStatus();
    // Reset tracking ref on mount (every tab visit)
    hasTrackedViewRef.current = false;
  }, [loadPreferences, loadOpenClawStatus]);

  // Track preferences viewed analytics on every tab visit (once per mount)
  useEffect(() => {
    if (hasTrackedViewRef.current) return;

    const trackPreferencesViewed = async () => {
      // Wait for notification settings to be available (either from cache or after loading)
      if (notificationSettings) {
        await Analytics.track('preferences_viewed', {
          notifications_enabled: notificationSettings.notification_preferences.show_recording_started ? 'true' : 'false'
        });
        hasTrackedViewRef.current = true;
      } else if (!isLoadingPreferences) {
        // If not loading and no settings available, track with default value
        await Analytics.track('preferences_viewed', {
          notifications_enabled: 'false'
        });
        hasTrackedViewRef.current = true;
      }
    };

    trackPreferencesViewed();
  }, [notificationSettings, isLoadingPreferences]);

  // Update notificationsEnabled when notificationSettings are loaded from global state
  useEffect(() => {
    if (notificationSettings) {
      // Notification enabled means both started and stopped notifications are enabled
      const enabled =
        notificationSettings.notification_preferences.show_recording_started &&
        notificationSettings.notification_preferences.show_recording_stopped;
      setNotificationsEnabled(enabled);
      if (isInitialLoad) {
        setPreviousNotificationsEnabled(enabled);
        setIsInitialLoad(false);
      }
    } else if (!isLoadingPreferences) {
      // If not loading and no settings, use default
      setNotificationsEnabled(true);
      if (isInitialLoad) {
        setPreviousNotificationsEnabled(true);
        setIsInitialLoad(false);
      }
    }
  }, [notificationSettings, isLoadingPreferences, isInitialLoad])

  useEffect(() => {
    // Skip update on initial load or if value hasn't actually changed
    if (isInitialLoad || notificationsEnabled === null || notificationsEnabled === previousNotificationsEnabled) return;
    if (!notificationSettings) return;

    const handleUpdateNotificationSettings = async () => {
      console.log("Updating notification settings to:", notificationsEnabled);

      try {
        // Update the notification preferences
        const updatedSettings: NotificationSettings = {
          ...notificationSettings,
          notification_preferences: {
            ...notificationSettings.notification_preferences,
            show_recording_started: notificationsEnabled,
            show_recording_stopped: notificationsEnabled,
          }
        };

        console.log("Calling updateNotificationSettings with:", updatedSettings);
        await updateNotificationSettings(updatedSettings);
        setPreviousNotificationsEnabled(notificationsEnabled);
        console.log("Successfully updated notification settings to:", notificationsEnabled);

        // Track notification preference change - only fires when user manually toggles
        await Analytics.track('notification_settings_changed', {
          notifications_enabled: notificationsEnabled.toString()
        });
      } catch (error) {
        console.error('Failed to update notification settings:', error);
      }
    };

    handleUpdateNotificationSettings();
  }, [notificationsEnabled, notificationSettings, isInitialLoad, previousNotificationsEnabled, updateNotificationSettings])

  const handleOpenFolder = async (folderType: 'database' | 'models' | 'recordings') => {
    try {
      switch (folderType) {
        case 'database':
          await invoke('open_database_folder');
          break;
        case 'models':
          await invoke('open_models_folder');
          break;
        case 'recordings':
          await invoke('open_recordings_folder');
          break;
      }

      // Track storage folder access
      await Analytics.track('storage_folder_opened', {
        folder_type: folderType
      });
    } catch (error) {
      console.error(`Failed to open ${folderType} folder:`, error);
    }
  };

  // Show loading only if we're actually loading and don't have cached data
  if (isLoadingPreferences && !notificationSettings && !storageLocations) {
    return <div className="max-w-2xl mx-auto p-6">Loading Preferences...</div>
  }

  // Show loading if notificationsEnabled hasn't been determined yet
  if (notificationsEnabled === null && !isLoadingPreferences) {
    return <div className="max-w-2xl mx-auto p-6">Loading Preferences...</div>
  }

  // Ensure we have a boolean value for the Switch component
  const notificationsEnabledValue = notificationsEnabled ?? false;

  return (
    <div className="space-y-6">
      {/* Notifications Section */}
      <div className="bg-white rounded-lg border border-gray-200 p-6 shadow-sm">
        <div className="flex items-center justify-between">
          <div>
            <h3 className="text-lg font-semibold text-gray-900 mb-2">Notifications</h3>
            <p className="text-sm text-gray-600">Enable or disable notifications of start and end of meeting</p>
          </div>
          <Switch checked={notificationsEnabledValue} onCheckedChange={setNotificationsEnabled} />
        </div>
      </div>

      <div className="bg-white rounded-lg border border-gray-200 p-6 shadow-sm">
        <div className="flex items-start justify-between gap-4">
          <div className="flex items-start gap-3">
            <div className="mt-1 rounded-md bg-slate-100 p-2 text-slate-700">
              <ServerCog className="h-5 w-5" />
            </div>
            <div>
              <h3 className="text-lg font-semibold text-gray-900 mb-2">OpenClaw Handoff</h3>
              <div className="flex flex-wrap items-center gap-2">
                <span className={`rounded-full px-2.5 py-1 text-xs font-medium ${openClawStatus?.ready ? 'bg-green-100 text-green-800' : 'bg-amber-100 text-amber-800'}`}>
                  {openClawStatus?.ready ? 'Ready' : 'Not ready'}
                </span>
                <span className="text-sm text-gray-600">
                  {openClawStatus?.status_message ?? (openClawStatusError ? 'Status unavailable' : 'Loading status')}
                </span>
              </div>
            </div>
          </div>
          <button
            onClick={() => void loadOpenClawStatus()}
            disabled={isOpenClawStatusLoading}
            className="inline-flex h-9 w-9 items-center justify-center rounded-md border border-gray-300 text-gray-600 transition-colors hover:bg-gray-50 disabled:opacity-50"
            aria-label="Refresh OpenClaw handoff status"
            title="Refresh OpenClaw handoff status"
          >
            <RefreshCw className={`h-4 w-4 ${isOpenClawStatusLoading ? 'animate-spin' : ''}`} />
          </button>
        </div>

        {openClawStatusError ? (
          <div className="mt-4 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-700">
            {openClawStatusError}
          </div>
        ) : (
          <div className="mt-5 grid gap-3 text-sm md:grid-cols-2">
            <div>
              <div className="text-xs font-medium uppercase text-gray-500">Endpoint</div>
              <div className="mt-1 break-all font-mono text-xs text-gray-800">
                {openClawStatus?.endpoint ?? 'Loading...'}
              </div>
            </div>
            <div>
              <div className="text-xs font-medium uppercase text-gray-500">Source</div>
              <div className="mt-1 font-mono text-xs text-gray-800">
                {openClawStatus?.source ?? 'Loading...'}
              </div>
            </div>
            <div>
              <div className="text-xs font-medium uppercase text-gray-500">Bearer Token</div>
              <div className="mt-1 text-gray-800">
                {openClawStatus?.bearer_token_configured ? 'Configured' : 'Missing'}
              </div>
            </div>
            <div>
              <div className="text-xs font-medium uppercase text-gray-500">Audio Path</div>
              <div className="mt-1 text-gray-800">
                {openClawStatus?.include_audio_path ? 'Included' : 'Not included'}
              </div>
            </div>
            <div className="md:col-span-2">
              <div className="text-xs font-medium uppercase text-gray-500">Config File</div>
              <div className="mt-1 break-all font-mono text-xs text-gray-800">
                {openClawStatus?.config_path ?? 'Loading...'}
              </div>
            </div>
            {openClawStatus?.last_submission && (
              <div className="md:col-span-2 rounded-md bg-gray-50 p-3">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="text-xs font-medium uppercase text-gray-500">Last Handoff</span>
                  <span className="rounded-full bg-gray-200 px-2 py-0.5 text-xs font-medium text-gray-700">
                    {openClawStatus.last_submission.state}
                  </span>
                  {openClawStatus.last_submission.status_code && (
                    <span className="text-xs text-gray-500">HTTP {openClawStatus.last_submission.status_code}</span>
                  )}
                </div>
                <div className="mt-2 text-sm text-gray-700">{openClawStatus.last_submission.message}</div>
                <div className="mt-1 text-xs text-gray-500">{openClawStatus.last_submission.updated_at}</div>
              </div>
            )}
          </div>
        )}
      </div>

      {/* Data Storage Locations Section */}
      <div className="bg-white rounded-lg border border-gray-200 p-6 shadow-sm">
        <h3 className="text-lg font-semibold text-gray-900 mb-4">Data Storage Locations</h3>
        <p className="text-sm text-gray-600 mb-6">
          View and access where Meetily stores your data
        </p>

        <div className="space-y-4">
          {/* Database Location */}
          {/* <div className="p-4 border rounded-lg bg-gray-50">
            <div className="font-medium mb-2">Database</div>
            <div className="text-sm text-gray-600 mb-3 break-all font-mono text-xs">
              {storageLocations?.database || 'Loading...'}
            </div>
            <button
              onClick={() => handleOpenFolder('database')}
              className="flex items-center gap-2 px-3 py-2 text-sm border border-gray-300 rounded-md hover:bg-gray-100 transition-colors"
            >
              <FolderOpen className="w-4 h-4" />
              Open Folder
            </button>
          </div> */}

          {/* Models Location */}
          {/* <div className="p-4 border rounded-lg bg-gray-50">
            <div className="font-medium mb-2">Whisper Models</div>
            <div className="text-sm text-gray-600 mb-3 break-all font-mono text-xs">
              {storageLocations?.models || 'Loading...'}
            </div>
            <button
              onClick={() => handleOpenFolder('models')}
              className="flex items-center gap-2 px-3 py-2 text-sm border border-gray-300 rounded-md hover:bg-gray-100 transition-colors"
            >
              <FolderOpen className="w-4 h-4" />
              Open Folder
            </button>
          </div> */}

          {/* Recordings Location */}
          <div className="p-4 border rounded-lg bg-gray-50">
            <div className="font-medium mb-2">Meeting Recordings</div>
            <div className="text-sm text-gray-600 mb-3 break-all font-mono text-xs">
              {storageLocations?.recordings || 'Loading...'}
            </div>
            <button
              onClick={() => handleOpenFolder('recordings')}
              className="flex items-center gap-2 px-3 py-2 text-sm border border-gray-300 rounded-md hover:bg-gray-100 transition-colors"
            >
              <FolderOpen className="w-4 h-4" />
              Open Folder
            </button>
          </div>
        </div>

        <div className="mt-4 p-3 bg-blue-50 rounded-md">
          <p className="text-xs text-blue-800">
            <strong>Note:</strong> Database and models are stored together in your application data directory for unified management.
          </p>
        </div>
      </div>

      {/* Analytics Section */}
      <div className="bg-white rounded-lg border border-gray-200 p-6 shadow-sm">
        <AnalyticsConsentSwitch />
      </div>
    </div>
  )
}
