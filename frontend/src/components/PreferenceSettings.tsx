"use client";

import { useEffect, useState, useRef } from "react";
import { Switch } from "./ui/switch";
import { FolderOpen, RefreshCw } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import Analytics from "@/lib/analytics";
import { useConfig, NotificationSettings } from "@/contexts/ConfigContext";
import { ThemeSettings } from "./ThemeSettings";
import { KeyboardShortcutsSettings } from "./KeyboardShortcutsSettings";
import {
  getAutoUpdateCheckEnabled,
  setAutoUpdateCheckEnabled,
} from "@/lib/updatePreferences";

export function PreferenceSettings() {
  const {
    notificationSettings,
    storageLocations,
    isLoadingPreferences,
    loadPreferences,
    updateNotificationSettings,
  } = useConfig();

  const [notificationsEnabled, setNotificationsEnabled] = useState<
    boolean | null
  >(null);
  const [isInitialLoad, setIsInitialLoad] = useState(true);
  const [previousNotificationsEnabled, setPreviousNotificationsEnabled] =
    useState<boolean | null>(null);
  const [autoUpdateChecksEnabled, setAutoUpdateChecksEnabled] = useState(true);
  const hasTrackedViewRef = useRef(false);

  // Lazy load preferences on mount (only loads if not already cached)
  useEffect(() => {
    loadPreferences();
    setAutoUpdateChecksEnabled(getAutoUpdateCheckEnabled());
    // Reset tracking ref on mount (every tab visit)
    hasTrackedViewRef.current = false;
  }, [loadPreferences]);

  // Track preferences viewed analytics on every tab visit (once per mount)
  useEffect(() => {
    if (hasTrackedViewRef.current) return;

    const trackPreferencesViewed = async () => {
      // Wait for notification settings to be available (either from cache or after loading)
      if (notificationSettings) {
        await Analytics.track("preferences_viewed", {
          notifications_enabled: notificationSettings.notification_preferences
            .show_recording_started
            ? "true"
            : "false",
        });
        hasTrackedViewRef.current = true;
      } else if (!isLoadingPreferences) {
        // If not loading and no settings available, track with default value
        await Analytics.track("preferences_viewed", {
          notifications_enabled: "false",
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
  }, [notificationSettings, isLoadingPreferences, isInitialLoad]);

  useEffect(() => {
    // Skip update on initial load or if value hasn't actually changed
    if (
      isInitialLoad ||
      notificationsEnabled === null ||
      notificationsEnabled === previousNotificationsEnabled
    )
      return;
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
          },
        };

        console.log(
          "Calling updateNotificationSettings with:",
          updatedSettings,
        );
        await updateNotificationSettings(updatedSettings);
        setPreviousNotificationsEnabled(notificationsEnabled);
        console.log(
          "Successfully updated notification settings to:",
          notificationsEnabled,
        );

        // Track notification preference change - only fires when user manually toggles
        await Analytics.track("notification_settings_changed", {
          notifications_enabled: notificationsEnabled.toString(),
        });
      } catch (error) {
        console.error("Failed to update notification settings:", error);
      }
    };

    handleUpdateNotificationSettings();
  }, [
    notificationsEnabled,
    notificationSettings,
    isInitialLoad,
    previousNotificationsEnabled,
    updateNotificationSettings,
  ]);

  const handleOpenFolder = async (
    folderType: "database" | "models" | "recordings",
  ) => {
    try {
      switch (folderType) {
        case "database":
          await invoke("open_database_folder");
          break;
        case "models":
          await invoke("open_models_folder");
          break;
        case "recordings":
          await invoke("open_recordings_folder");
          break;
      }

      // Track storage folder access
      await Analytics.track("storage_folder_opened", {
        folder_type: folderType,
      });
    } catch (error) {
      console.error(`Failed to open ${folderType} folder:`, error);
    }
  };

  const handleAutoUpdateCheckChange = (enabled: boolean) => {
    setAutoUpdateChecksEnabled(enabled);
    setAutoUpdateCheckEnabled(enabled);
    Analytics.track("update_settings_changed", {
      automatic_launch_check: enabled.toString(),
    }).catch((error) => {
      console.error("Failed to track update preference change:", error);
    });
  };

  // Show loading only if we're actually loading and don't have cached data
  if (isLoadingPreferences && !notificationSettings && !storageLocations) {
    return <div className="max-w-2xl mx-auto p-6">Loading Preferences...</div>;
  }

  // Show loading if notificationsEnabled hasn't been determined yet
  if (notificationsEnabled === null && !isLoadingPreferences) {
    return <div className="max-w-2xl mx-auto p-6">Loading Preferences...</div>;
  }

  // Ensure we have a boolean value for the Switch component
  const notificationsEnabledValue = notificationsEnabled ?? false;

  return (
    // Ordered most-used first (appearance, notifications), then shortcuts, with
    // data/advanced (storage) last.
    <div className="space-y-5">
      <ThemeSettings />

      <div className="rounded-md border border-border bg-card p-6 shadow-sm">
        <div className="flex items-center justify-between gap-6">
          <div className="flex min-w-0 items-start gap-3">
            <span className="mt-0.5 flex h-9 w-9 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary">
              <RefreshCw className="h-4 w-4" />
            </span>
            <div>
              <h3 className="text-lg font-semibold text-foreground">
                Updates
              </h3>
              <p className="mt-2 text-sm text-muted-foreground">
                Check for new ClawScribe releases when the app starts.
              </p>
            </div>
          </div>
          <Switch
            checked={autoUpdateChecksEnabled}
            onCheckedChange={handleAutoUpdateCheckChange}
            aria-label="Check for updates at launch"
          />
        </div>
      </div>

      {/* Notifications Section */}
      <div className="rounded-md border border-border bg-card p-6 shadow-sm">
        <div className="flex items-center justify-between">
          <div>
            <h3 className="text-lg font-semibold text-foreground mb-2">
              Notifications
            </h3>
            <p className="text-sm text-muted-foreground">
              Enable or disable notifications of start and end of meeting
            </p>
          </div>
          <Switch
            checked={notificationsEnabledValue}
            onCheckedChange={setNotificationsEnabled}
          />
        </div>
      </div>

      <KeyboardShortcutsSettings />

      {/* Data Storage Locations Section */}
      <div className="rounded-md border border-border bg-card p-6 shadow-sm">
        <h3 className="text-lg font-semibold text-foreground mb-4">
          Data Storage Locations
        </h3>
        <p className="text-sm text-muted-foreground mb-6">
          View and access where ClawScribe stores your data
        </p>

        <div className="space-y-4">
          {/* Database Location */}
          {/* <div className="p-4 border rounded-lg bg-muted">
            <div className="font-medium mb-2">Database</div>
            <div className="text-sm text-muted-foreground mb-3 break-all font-mono text-xs">
              {storageLocations?.database || 'Loading...'}
            </div>
            <button
              onClick={() => handleOpenFolder('database')}
              className="flex items-center gap-2 px-3 py-2 text-sm border border-border rounded-lg hover:bg-muted transition-colors"
            >
              <FolderOpen className="w-4 h-4" />
              Open Folder
            </button>
          </div> */}

          {/* Models Location */}
          {/* <div className="p-4 border rounded-lg bg-muted">
            <div className="font-medium mb-2">Whisper Models</div>
            <div className="text-sm text-muted-foreground mb-3 break-all font-mono text-xs">
              {storageLocations?.models || 'Loading...'}
            </div>
            <button
              onClick={() => handleOpenFolder('models')}
              className="flex items-center gap-2 px-3 py-2 text-sm border border-border rounded-lg hover:bg-muted transition-colors"
            >
              <FolderOpen className="w-4 h-4" />
              Open Folder
            </button>
          </div> */}

          {/* Recordings Location */}
          <div className="p-4 border rounded-md bg-muted">
            <div className="font-medium mb-2">Meeting Recordings</div>
            <div className="text-sm text-muted-foreground mb-3 break-all font-mono text-xs">
              {storageLocations?.recordings || "Loading..."}
            </div>
            <button
              onClick={() => handleOpenFolder("recordings")}
              className="flex items-center gap-2 px-3 py-2 text-sm border border-border rounded-md hover:bg-muted transition-colors"
            >
              <FolderOpen className="w-4 h-4" />
              Open Folder
            </button>
          </div>
        </div>

        <div className="mt-4 rounded-md bg-muted p-3">
          <p className="text-xs text-foreground">
            <strong>Note:</strong> Database and models are stored together in
            your application data directory for unified management.
          </p>
        </div>
      </div>
    </div>
  );
}
