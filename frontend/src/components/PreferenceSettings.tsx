"use client";

import { useEffect, useRef, useState } from "react";
import { FolderOpen, Loader2 } from "lucide-react";
import { toast } from "sonner";
import { invoke } from "@tauri-apps/api/core";
import { useRouter } from "next/navigation";
import { Button } from "./ui/button";
import { Switch } from "./ui/switch";
import { Meta, SettingsCard, SettingsRow, SettingsStack } from "./ui/settings-layout";
import { NoteRow } from "./settings/settings-parts";
import { UpdateChannelSettings } from "./UpdateChannelSettings";
import { PendingRecordingNotes } from "./PendingRecordingNotes";
import { LibraryBackup } from "./LibraryBackup";
import { ArchiveStorageButton } from "./Archive/ArchiveStorageButton";
import Analytics from "@/lib/analytics";
import { usePreferenceConfig, type NotificationSettings } from "@/contexts/ConfigContext";
import { AUTO_UPDATE_CHECK_CHANGED_EVENT, loadAutoUpdateCheckEnabled, setAutoUpdateCheckEnabled } from "@/lib/updatePreferences";
import { useRecordingState } from "@/contexts/RecordingStateContext";
import { useInterruptedRecordings } from "@/hooks/useInterruptedRecordings";
import { RECOVERY_OPEN_ALL, RECOVERY_OPEN_REQUEST } from "@/lib/interruptedRecordings";

type FolderKind = "database" | "models" | "recordings";

interface StorageRow {
  kind: FolderKind;
  title: string;
  path: string | undefined;
}

const normalizePath = (path: string) => path.replace(/[\\/]+$/, "").replace(/\\/g, "/").toLowerCase();

/**
 * The locations the app really exposes. The recordings folder is the one the
 * user chose. Models are listed on their own only when they live outside the
 * database folder.
 */
export function storageRows(
  recordingsPath: string | undefined,
  locations: { database: string; models: string } | null,
): StorageRow[] {
  const rows: StorageRow[] = [{ kind: "recordings", title: "Recordings", path: recordingsPath }];
  if (!locations) return rows;
  const modelsInsideDatabase = !!locations.models
    && normalizePath(locations.models).startsWith(normalizePath(locations.database));
  rows.push({ kind: "database", title: modelsInsideDatabase ? "Database and models" : "Database", path: locations.database });
  if (locations.models && !modelsInsideDatabase) rows.push({ kind: "models", title: "Whisper models", path: locations.models });
  return rows;
}

export function PreferenceSettings() {
  const {
    notificationSettings,
    storageLocations,
    isLoadingPreferences,
    loadPreferences,
    updateNotificationSettings,
  } = usePreferenceConfig();

  const router = useRouter();
  const recordingState = useRecordingState();
  // Interrupted recordings (found in the browser data and on disk) are shown
  // only when there are some, and never while a recording is running.
  const interrupted = useInterruptedRecordings(!recordingState.isRecording);
  const reviewInterrupted = (includeKept = false) => {
    try { sessionStorage.setItem(RECOVERY_OPEN_REQUEST, includeKept ? RECOVERY_OPEN_ALL : "true"); } catch { /* the home page still offers them on start */ }
    router.push("/");
  };

  const [loaded, setLoaded] = useState(false);
  const [autoUpdateChecksEnabled, setAutoUpdateChecksEnabled] = useState<boolean | null>(null);
  const savingUpdatePreference = useRef(false);
  const [updatingPreference, setUpdatingPreference] = useState(false);
  const [savingNotifications, setSavingNotifications] = useState(false);
  const [notificationError, setNotificationError] = useState<string | null>(null);
  const [saveFolder, setSaveFolder] = useState<string | undefined>(undefined);
  const [openingFolder, setOpeningFolder] = useState<FolderKind | null>(null);
  const [choosingFolder, setChoosingFolder] = useState(false);
  const folderPicker = useRef(false);
  const hasTrackedViewRef = useRef(false);

  // Lazy load preferences on mount (only loads if not already cached).
  useEffect(() => {
    let active = true;
    const syncUpdatePreference = () => {
      void loadAutoUpdateCheckEnabled().then(enabled => { if (active) setAutoUpdateChecksEnabled(enabled); });
    };
    window.addEventListener(AUTO_UPDATE_CHECK_CHANGED_EVENT, syncUpdatePreference);
    syncUpdatePreference();
    hasTrackedViewRef.current = false;
    void loadPreferences().finally(() => { if (active) setLoaded(true); });
    invoke<{ save_folder?: string }>("get_recording_preferences")
      .then((preferences) => { if (active && preferences?.save_folder) setSaveFolder(preferences.save_folder); })
      .catch(() => { /* the default folder from the app is shown instead */ });
    return () => { active = false; window.removeEventListener(AUTO_UPDATE_CHECK_CHANGED_EVENT, syncUpdatePreference); };
  }, [loadPreferences]);

  // Track preferences viewed once per visit, when the settings are known.
  useEffect(() => {
    if (hasTrackedViewRef.current || !loaded) return;
    hasTrackedViewRef.current = true;
    void Analytics.track("preferences_viewed", {
      notifications_enabled: notificationSettings?.notification_preferences.show_recording_started ? "true" : "false",
    });
  }, [loaded, notificationSettings]);

  const notificationsEnabled = notificationSettings
    ? notificationSettings.notification_preferences.show_recording_started
      && notificationSettings.notification_preferences.show_recording_stopped
    : false;
  const notificationsLoading = !loaded || isLoadingPreferences;

  const changeNotifications = async (enabled: boolean) => {
    if (!notificationSettings || savingNotifications) return;
    setSavingNotifications(true);
    setNotificationError(null);
    try {
      const updated: NotificationSettings = {
        ...notificationSettings,
        notification_preferences: {
          ...notificationSettings.notification_preferences,
          show_recording_started: enabled,
          show_recording_stopped: enabled,
        },
      };
      await updateNotificationSettings(updated);
      Analytics.track("notification_settings_changed", { notifications_enabled: enabled.toString() })
        .catch((error) => console.error("Failed to track notification change:", error));
    } catch (error) {
      console.error("Failed to update notification settings:", error);
      setNotificationError("Could not save the notification setting. Try again.");
    } finally {
      setSavingNotifications(false);
    }
  };

  const handleOpenFolder = async (folderType: FolderKind) => {
    setOpeningFolder(folderType);
    try {
      await invoke(`open_${folderType}_folder`);
      Analytics.track("storage_folder_opened", { folder_type: folderType })
        .catch((error) => console.error("Failed to track folder open:", error));
    } catch (error) {
      console.error(`Failed to open ${folderType} folder:`, error);
      toast.error("Could not open the folder.");
    } finally {
      setOpeningFolder(null);
    }
  };

  const handleAutoUpdateCheckChange = async (enabled: boolean) => {
    if (savingUpdatePreference.current) return;
    savingUpdatePreference.current = true;
    setUpdatingPreference(true);
    try {
      await setAutoUpdateCheckEnabled(enabled);
      setAutoUpdateChecksEnabled(enabled);
    } catch {
      toast.error("Could not save the automatic update setting. Try again.");
    } finally {
      savingUpdatePreference.current = false;
      setUpdatingPreference(false);
    }
  };

  const chooseFolder = async () => {
    if (folderPicker.current) return;
    folderPicker.current = true;
    setChoosingFolder(true);
    try {
      const folder = await invoke<string | null>("select_recording_folder");
      if (!folder) return;
      await invoke("set_recording_save_folder", { folder });
      setSaveFolder(folder);
      toast.success("Recording folder updated");
    } catch (error) {
      toast.error("Could not choose the recordings folder", { description: typeof error === "string" ? error : undefined });
    } finally {
      folderPicker.current = false;
      setChoosingFolder(false);
    }
  };

  const rows = storageRows(saveFolder ?? storageLocations?.recordings, storageLocations);
  const locationsUnavailable = loaded && !isLoadingPreferences && !storageLocations;

  return (
    <SettingsStack>
      <SettingsCard title="Updates">
        <SettingsRow
          title="Check for updates automatically"
          htmlFor="preferences-auto-update"
          description="Checks at startup and while ClawScribe is running in the tray"
        >
          <Switch id="preferences-auto-update" checked={autoUpdateChecksEnabled ?? true} disabled={autoUpdateChecksEnabled === null || updatingPreference} onCheckedChange={handleAutoUpdateCheckChange} />
        </SettingsRow>
        <UpdateChannelSettings id="preferences-prerelease-updates" />
      </SettingsCard>

      <SettingsCard title="Notifications">
        <SettingsRow
          title="Meeting start and end"
          htmlFor="preferences-notifications"
          description="Shows a Windows notification when a recording starts and when it ends"
        >
          {(notificationsLoading || savingNotifications) && (
            <span role="status" className="inline-flex items-center gap-2 text-[13px] text-muted-foreground">
              <Loader2 aria-hidden="true" className="h-3.5 w-3.5 animate-spin" />
              {savingNotifications ? "Saving…" : "Loading…"}
            </span>
          )}
          <Switch
            id="preferences-notifications"
            checked={notificationsEnabled}
            disabled={notificationsLoading || savingNotifications || !notificationSettings}
            onCheckedChange={(enabled) => void changeNotifications(enabled)}
          />
        </SettingsRow>
        {!notificationsLoading && !notificationSettings && (
          <NoteRow tone="muted">Notification settings could not be read, so they cannot be changed here.</NoteRow>
        )}
        {notificationError && <NoteRow tone="error">{notificationError}</NoteRow>}
      </SettingsCard>

      <SettingsCard title="Storage">
        {rows.map((row) => (
          <SettingsRow
            key={row.kind}
            title={row.title}
            description={
              row.path
                ? <Meta className="whitespace-normal break-all" >{row.path}</Meta>
                : <span role="status">{locationsUnavailable ? "Location unavailable" : "Loading…"}</span>
            }
          >
            <Button
              type="button"
              variant="outline"
              size="sm"
              disabled={openingFolder !== null}
              aria-label={`Open the ${row.title.toLowerCase()} folder`}
              onClick={() => void handleOpenFolder(row.kind)}
            >
              {openingFolder === row.kind ? <Loader2 aria-hidden="true" className="animate-spin" /> : <FolderOpen aria-hidden="true" />}
              {row.kind === "recordings" ? "Open recordings folder" : "Open folder"}
            </Button>
            {row.kind === "recordings" && <Button type="button" size="sm" variant="outline"
              disabled={choosingFolder || recordingState.isRecording || recordingState.isStarting || recordingState.isStopping}
              onClick={() => void chooseFolder()}>
              {choosingFolder ? "Choosing…" : "Choose folder"}
            </Button>}
          </SettingsRow>
        ))}
        <SettingsRow title="Library storage" description="See disk usage by meeting and review pending file cleanup.">
          <ArchiveStorageButton label="Review storage" />
        </SettingsRow>
        <SettingsRow title="Backup and restore" description="Save or restore a portable copy of your local meeting library.">
          <LibraryBackup />
        </SettingsRow>
        {locationsUnavailable && <NoteRow tone="muted">The database and model locations could not be read.</NoteRow>}
        {(interrupted.scanFailed || interrupted.hasUnverifiedFolders || interrupted.waitingForAudio) && (
          <SettingsRow title="Recovery scan" description={interrupted.scanFailed
            ? "The recovery scan failed. Your drafts are kept."
            : interrupted.hasUnverifiedFolders
              ? "Some recording folders could not be verified. Reconnect the recordings location if needed, then retry."
              : "Check for audio from recordings whose transcripts you saved."}>
            <Button type="button" variant="outline" size="sm" disabled={interrupted.loading} onClick={interrupted.refresh}>
              {interrupted.loading ? 'Checking…' : 'Retry scan'}
            </Button>
          </SettingsRow>
        )}
        {(interrupted.count > 0 || interrupted.keptCount > 0) && (
          <SettingsRow
            title="Interrupted recordings"
            description={
              interrupted.count > 0
                ? `${interrupted.count} ${interrupted.count === 1 ? "recording was" : "recordings were"} interrupted. Review available recovery options${interrupted.keptCount > 0 ? `. ${interrupted.keptCount} more ${interrupted.keptCount === 1 ? "is" : "are"} kept and not offered` : ""}`
                : `${interrupted.keptCount} ${interrupted.keptCount === 1 ? "recording is" : "recordings are"} kept and not offered`
            }
          >
            {interrupted.count > 0 && (
              <Button type="button" variant="outline" size="sm" onClick={() => reviewInterrupted(false)}>
                Review ({interrupted.count})
              </Button>
            )}
            {interrupted.keptCount > 0 && (
              <Button type="button" variant="ghost" size="sm" onClick={() => reviewInterrupted(true)}>
                Show kept ({interrupted.keptCount})
              </Button>
            )}
          </SettingsRow>
        )}
        <SettingsRow title="Pending recording notes" description="Review local notes and summary preferences for recordings awaiting recovery">
          <PendingRecordingNotes disabled={recordingState.isRecording} />
        </SettingsRow>
      </SettingsCard>
    </SettingsStack>
  );
}
