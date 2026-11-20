import React, { useState, useEffect } from 'react';
import { Switch } from '@/components/ui/switch';
import { Button } from '@/components/ui/button';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Meta, SettingsCard, SettingsColumns, SettingsRow } from '@/components/ui/settings-layout';
import { FolderOpen } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import type { SelectedDevices } from '@/components/DeviceSelection';
import { AudioDevicesCard } from '@/components/settings/AudioDevicesCard';
import { CallDetectionSettings } from '@/components/settings/CallDetectionSettings';
import { SignalCheck } from '@/components/settings/SignalCheck';
import { useAudioLevelFeed } from '@/components/settings/useAudioLevelFeed';
import Analytics from '@/lib/analytics';
import { toast } from 'sonner';
import { setSourceAttribution } from '@/lib/sourceAttribution';
import { useConfig } from '@/contexts/ConfigContext';
import { selectedDevicesFromPreferences } from '@/lib/audioDevicePreferences';
import { RecordingMode, useRecordingState } from '@/contexts/RecordingStateContext';

export interface RecordingPreferences {
  save_folder: string;
  auto_save: boolean;
  file_format: string;
  preferred_mic_device: string | null;
  preferred_system_device: string | null;
}

interface RecordingSettingsProps {
  onSave?: (preferences: RecordingPreferences) => void;
}

const UNSELECTED_RECORDING_MODE = '__unset_recording_mode__';

export function RecordingSettings({ onSave }: RecordingSettingsProps) {
  const { setSelectedDevices } = useConfig();
  const recording = useRecordingState();
  const { recordingMode } = recording;
  const audioOnly = recordingMode === 'audio_only';
  const recordingBusy = recording.isRecording || recording.isStarting || recording.isStopping;
  const [preferences, setPreferences] = useState<RecordingPreferences>({
    save_folder: '',
    auto_save: true,
    file_format: 'mp4',
    preferred_mic_device: null,
    preferred_system_device: null
  });
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [showRecordingNotification, setShowRecordingNotification] = useState(true);

  // The signal check runs only while this page is open and nothing is being
  // recorded. The hook stops the native monitor when the page is left.
  const levelFeed = useAudioLevelFeed({
    enabled: !loading && !recordingBusy,
    micDevice: preferences.preferred_mic_device,
  });

  // Load recording preferences on component mount
  useEffect(() => {
    const loadPreferences = async () => {
      try {
        const prefs = await invoke<RecordingPreferences>('get_recording_preferences');
        setPreferences(prefs);
        setSelectedDevices(selectedDevicesFromPreferences(prefs));
      } catch (error) {
        console.error('Failed to load recording preferences:', error);
        // If loading fails, get default folder path
        try {
          const defaultPath = await invoke<string>('get_default_recordings_folder_path');
          setPreferences(prev => ({ ...prev, save_folder: defaultPath }));
        } catch (defaultError) {
          console.error('Failed to get default folder path:', defaultError);
        }
      } finally {
        setLoading(false);
      }
    };

    loadPreferences();
  }, [setSelectedDevices]);

  // Load recording notification preference
  useEffect(() => {
    const loadNotificationPref = async () => {
      try {
        const { Store } = await import('@tauri-apps/plugin-store');
        const store = await Store.load('preferences.json');
        const show = await store.get<boolean>('show_recording_notification') ?? true;
        setShowRecordingNotification(show);
      } catch (error) {
        console.error('Failed to load notification preference:', error);
      }
    };
    loadNotificationPref();
  }, []);

  const handleAutoSaveToggle = async (enabled: boolean) => {
    const newPreferences = { ...preferences, auto_save: enabled };
    setPreferences(newPreferences);
    await savePreferences(newPreferences, {
      successTitle: enabled ? 'Audio saving enabled' : 'Audio saving disabled'
    });
    if (!enabled) {
      await setSourceAttribution(false);
    }

    // Track auto-save setting change
    await Analytics.track('auto_save_recording_toggled', {
      enabled: enabled.toString()
    });
  };

  const handleDeviceChange = async (devices: SelectedDevices) => {
    const newPreferences = {
      ...preferences,
      preferred_mic_device: devices.micDevice,
      preferred_system_device: devices.systemDevice
    };
    setPreferences(newPreferences);
    const micDevice = newPreferences.preferred_mic_device || 'Default';
    const systemDevice = newPreferences.preferred_system_device || 'Default';
    await savePreferences(newPreferences, {
      successTitle: 'Device preferences saved',
      successDescription: `Microphone: ${micDevice}, System audio: ${systemDevice}`
    });

    // Track default device preference changes
    // Note: Individual device selection analytics are tracked in the device selects
    await Analytics.track('default_devices_changed', {
      has_preferred_microphone: (!!devices.micDevice).toString(),
      has_preferred_system_audio: (!!devices.systemDevice).toString()
    });
  };

  const handleOpenFolder = async () => {
    try {
      await invoke('open_recordings_folder');
    } catch (error) {
      console.error('Failed to open recordings folder:', error);
    }
  };

  const handleChooseFolder = async () => {
    try {
      const selectedFolder = await invoke<string | null>('select_recording_folder');
      if (!selectedFolder) {
        return;
      }

      const newPreferences = { ...preferences, save_folder: selectedFolder };
      setPreferences(newPreferences);
      await savePreferences(newPreferences, {
        successTitle: 'Save location updated',
        successDescription: selectedFolder
      });

      await Analytics.track('recording_save_folder_changed');
    } catch (error) {
      console.error('Failed to choose recordings folder:', error);
      toast.error('Failed to choose recordings folder', {
        description: error instanceof Error ? error.message : String(error)
      });
    }
  };

  const handleNotificationToggle = async (enabled: boolean) => {
    try {
      setShowRecordingNotification(enabled);
      const { Store } = await import('@tauri-apps/plugin-store');
      const store = await Store.load('preferences.json');
      await store.set('show_recording_notification', enabled);
      await store.save();
      toast.success('Preference saved');
      await Analytics.track('recording_notification_preference_changed', {
        enabled: enabled.toString()
      });
    } catch (error) {
      console.error('Failed to save notification preference:', error);
      toast.error('Failed to save preference');
    }
  };

  const savePreferences = async (
    prefs: RecordingPreferences,
    toastOptions?: {
      successTitle?: string;
      successDescription?: string;
      errorTitle?: string;
    }
  ) => {
    setSaving(true);
    try {
      await invoke('set_recording_preferences', { preferences: prefs });
      // Home, recording start, and Settings all consume the same device state.
      // Keep it in sync only after the backend accepted the preference update.
      setSelectedDevices(selectedDevicesFromPreferences(prefs));
      onSave?.(prefs);

      toast.success(toastOptions?.successTitle || "Recording preferences saved", {
        description: toastOptions?.successDescription
      });
    } catch (error) {
      console.error('Failed to save recording preferences:', error);
      toast.error(toastOptions?.errorTitle || "Failed to save recording preferences", {
        description: error instanceof Error ? error.message : String(error)
      });
    } finally {
      setSaving(false);
    }
  };

  if (loading) {
    return (
      <SettingsColumns aside={null}>
        <SettingsCard title="Capture" aria-busy="true">
          <SettingsRow title="Loading recording settings" role="status" />
        </SettingsCard>
      </SettingsColumns>
    );
  }

  const modeLocked = recordingBusy || recording.isSavingMode;
  const savesAudio = audioOnly || preferences.auto_save;

  return (
    <SettingsColumns aside={<SignalCheck feed={levelFeed} unavailableReason={recordingBusy ? 'Not available while recording' : null} />}>
      <SettingsCard title="Capture">
        <SettingsRow
          title="Recording mode"
          htmlFor="recording-mode"
          description={audioOnly
            ? 'Always saves audio. No speech model or automatic notes run. Transcribe from the saved meeting later'
            : 'Shows recognized speech while recording. Applies to the next recording'}
        >
          <Select
            value={recordingMode || UNSELECTED_RECORDING_MODE}
            disabled={modeLocked}
            onValueChange={value => void recording.setRecordingMode((value === UNSELECTED_RECORDING_MODE ? '' : value) as RecordingMode)}
          >
            <SelectTrigger id="recording-mode" className="w-80 max-w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {!recordingMode && <SelectItem value={UNSELECTED_RECORDING_MODE}>Choose recording mode</SelectItem>}
              <SelectItem value="live">Record with live transcription</SelectItem>
              <SelectItem value="audio_only">Record now, transcribe later</SelectItem>
            </SelectContent>
          </Select>
        </SettingsRow>
        {recording.modeError && <SettingsRow title={<span role="alert" className="text-destructive">{recording.modeError}</span>} />}

        <SettingsRow
          title="Save audio recordings"
          htmlFor="save-audio"
          description={audioOnly
            ? 'Audio-only mode always saves the recording for later transcription'
            : `Keeps an ${preferences.file_format.toUpperCase()} file when a recording stops`}
        >
          <Switch id="save-audio" checked={savesAudio} onCheckedChange={handleAutoSaveToggle} disabled={saving} />
        </SettingsRow>

        {savesAudio && (
          <SettingsRow
            title="Save location"
            description={<Meta className="whitespace-normal break-all">{preferences.save_folder || 'Default folder'}</Meta>}
          >
            <Button variant="outline" size="sm" onClick={handleChooseFolder} disabled={saving}>Choose folder</Button>
            <Button variant="outline" size="sm" onClick={handleOpenFolder}>
              <FolderOpen aria-hidden="true" />
              Open recordings folder
            </Button>
          </SettingsRow>
        )}

        <SettingsRow
          title="Recording start reminder"
          htmlFor="recording-start-reminder"
          description="Reminds you to tell participants that the meeting is recorded"
        >
          <Switch id="recording-start-reminder" checked={showRecordingNotification} onCheckedChange={handleNotificationToggle} />
        </SettingsRow>
      </SettingsCard>

      <AudioDevicesCard
        selectedDevices={{
          micDevice: preferences.preferred_mic_device,
          systemDevice: preferences.preferred_system_device
        }}
        onDeviceChange={handleDeviceChange}
        disabled={saving}
        feed={levelFeed}
        meterPaused={recordingBusy}
      />

      <CallDetectionSettings />
    </SettingsColumns>
  );
}
