import React, { useState, useEffect, useRef, useSyncExternalStore } from 'react';
import { Switch } from '@/components/ui/switch';
import { Button } from '@/components/ui/button';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Meta, SettingsCard, SettingsColumns, SettingsRow } from '@/components/ui/settings-layout';
import { FolderOpen } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { AudioDevicesCard } from '@/components/settings/AudioDevicesCard';
import { CallDetectionSettings } from '@/components/settings/CallDetectionSettings';
import { MicrophoneCheck, SignalCheck } from '@/components/settings/SignalCheck';
import { useSignalCheck } from '@/components/settings/useSignalCheck';
import { useAudioLevelFeed } from '@/components/settings/useAudioLevelFeed';
import { usePlatform } from '@/hooks/usePlatform';
import { toast } from 'sonner';
import { setSourceAttribution } from '@/lib/sourceAttribution';
import { useTranscriptionConfig } from '@/contexts/ConfigContext';
import { canEditLiveAudioSaving } from '@/lib/audioDevicePreferences';
import { RecordingMode, useRecordingState } from '@/contexts/RecordingStateContext';
import { errorText } from '@/lib/errorText';
import { getBehaviorSettings, loadBehaviorSettings, refreshBehaviorSettings, subscribeBehaviorSettings } from '@/lib/behaviorSettings';

export interface RecordingPreferences {
  save_folder: string;
  auto_save: boolean;
  auto_open_floating_controls: boolean;
  remember_floating_position: boolean;
  file_format: string;
  preferred_mic_device: string | null;
  preferred_system_device: string | null;
  /** Set by the core when the stored recordings folder is not trusted and the default is used. */
  folder_notice?: string | null;
}

interface RecordingSettingsProps {
  onSave?: (preferences: RecordingPreferences) => void;
}

const UNSELECTED_RECORDING_MODE = '__unset_recording_mode__';

export function RecordingSettings({ onSave }: RecordingSettingsProps) {
  const { selectedDevices, saveSelectedDevices, devicePreferencesStatus, isSavingDevicePreferences } = useTranscriptionConfig();
  const recording = useRecordingState();
  const { recordingMode } = recording;
  const audioOnly = recordingMode === 'audio_only';
  const recordingBusy = recording.isRecording || recording.isStarting || recording.isStopping || recording.isSaving || recording.isProcessing;
  const [preferences, setPreferences] = useState<RecordingPreferences>({
    save_folder: '',
    auto_save: true,
    auto_open_floating_controls: true,
    remember_floating_position: true,
    file_format: 'mp4',
    preferred_mic_device: null,
    preferred_system_device: null
  });
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const [preferencesFailed, setPreferencesFailed] = useState(false);
  const [showRecordingNotification, setShowRecordingNotification] = useState(true);

  const { showConversationMetrics } = useSyncExternalStore(subscribeBehaviorSettings, getBehaviorSettings, getBehaviorSettings);
  const [savingMetrics, setSavingMetrics] = useState(false);

  async function handleMetricsToggle(enabled: boolean) {
    setSavingMetrics(true);
    try {
      await invoke('set_conversation_metrics_preference', { enabled });
      await refreshBehaviorSettings();
    } catch {
      toast.error('Could not update conversation metrics');
    } finally {
      setSavingMetrics(false);
    }
  }

  const platform = usePlatform();
  const windows = platform === 'windows';
  const checkBlocked = recordingBusy ? 'Unavailable while recording or saving' :
    loading || devicePreferencesStatus === 'loading' ? 'Checking audio devices…' :
      preferencesFailed || devicePreferencesStatus !== 'ready' ? 'Audio devices are unavailable' : null;
  const levelFeed = useSignalCheck(windows && checkBlocked === null, selectedDevices.micDevice, selectedDevices.systemDevice);
  const microphoneFeed = useAudioLevelFeed({
    enabled: (platform === 'macos' || platform === 'linux') && checkBlocked === null,
    micDevice: selectedDevices.micDevice,
  });
  const microphoneLevel = checkBlocked ? null : windows
    ? levelFeed.running ? levelFeed.levels.microphone : null
    : microphoneFeed.microphone;

  // Load recording preferences on component mount
  useEffect(() => {
    const loadPreferences = async () => {
      try {
        const prefs = await invoke<RecordingPreferences>('get_recording_preferences');
        setPreferences(prefs);
      } catch {
        setPreferencesFailed(true);
      } finally {
        setLoading(false);
      }
    };

    loadPreferences();
  }, []);

  // Load recording notification preference
  useEffect(() => {
    const loadNotificationPref = async () => {
      try {
        // The core owns the preference.
        setShowRecordingNotification((await loadBehaviorSettings()).showRecordingNotification);
      } catch (error) {
        console.error('Failed to load notification preference:', error);
      }
    };
    loadNotificationPref();
  }, []);

  const handleAutoSaveToggle = async (enabled: boolean) => {
    if (!canEditLiveAudioSaving(recordingMode) || recordingBusy || recording.isSavingMode) return;
    const saved = await savePreferences({ ...preferences, auto_save: enabled }, {
      successTitle: enabled ? 'Audio saving enabled' : 'Audio saving disabled'
    });
    if (!saved) return;
    if (!enabled) {
      try {
        await setSourceAttribution(false);
      } catch {
        toast.error('Could not update source attribution');
      }
    }
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
      const saved = await savePreferences(newPreferences, {
        successTitle: 'Save location updated',
        successDescription: selectedFolder
      });
      // The new folder is trusted: the notice about the earlier one no longer applies.
      if (saved) setPreferences(current => ({ ...current, folder_notice: null }));

    } catch (error) {
      console.error('Failed to choose recordings folder:', error);
      toast.error('Failed to choose recordings folder', {
        description: errorText(error)
      });
    }
  };

  const handleNotificationToggle = async (enabled: boolean) => {
    try {
      await invoke('set_recording_notification_preference', { enabled });
      await refreshBehaviorSettings();
      setShowRecordingNotification(enabled);
      toast.success('Preference saved');
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
    if (savingRef.current || preferencesFailed) return false;
    savingRef.current = true;
    setSaving(true);
    try {
      await invoke('set_recording_preferences', { preferences: prefs, preserveDevicePreferences: true });
      setPreferences(prefs);
      onSave?.(prefs);

      toast.success(toastOptions?.successTitle || "Recording preferences saved", {
        description: toastOptions?.successDescription
      });
      return true;
    } catch (error) {
      toast.error(toastOptions?.errorTitle || "Failed to save recording preferences", {
        description: errorText(error)
      });
      return false;
    } finally {
      savingRef.current = false;
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

  if (preferencesFailed) {
    return (
      <SettingsColumns aside={null}>
        <SettingsCard title="Capture">
          <SettingsRow title="Could not load recording settings" role="alert" description="Reopen Recording settings to try again." />
        </SettingsCard>
      </SettingsColumns>
    );
  }

  const modeLocked = recordingBusy || recording.isSavingMode || saving;
  const savesAudio = audioOnly || preferences.auto_save;

  return (
    <SettingsColumns aside={windows
      ? <SignalCheck feed={levelFeed} unavailableReason={checkBlocked} />
      : <MicrophoneCheck feed={microphoneFeed} unavailableReason={checkBlocked} />}>
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
          <Switch id="save-audio" checked={savesAudio} onCheckedChange={handleAutoSaveToggle} disabled={modeLocked || !canEditLiveAudioSaving(recordingMode)} />
        </SettingsRow>

        {savesAudio && (
          <SettingsRow
            title="Save location"
            description={<>
              <Meta className="whitespace-normal break-all">{preferences.save_folder || 'Default folder'}</Meta>
              {preferences.folder_notice && <span role="status" className="mt-1 block text-[13px] text-warning-foreground">{preferences.folder_notice}</span>}
            </>}
          >
            <Button variant="outline" size="sm" onClick={handleChooseFolder} disabled={saving}>Choose folder</Button>
            <Button variant="outline" size="sm" onClick={handleOpenFolder}>
              <FolderOpen aria-hidden="true" />
              Open recordings folder
            </Button>
          </SettingsRow>
        )}

        <SettingsRow
          title="Open floating controls when recording starts"
          htmlFor="auto-open-floating-controls"
          description="Applies to the next recording. You can also open the controls from the recording bar or tray."
        >
          <Switch
            id="auto-open-floating-controls"
            checked={preferences.auto_open_floating_controls}
            disabled={saving}
            onCheckedChange={enabled => void savePreferences({ ...preferences, auto_open_floating_controls: enabled })}
          />
        </SettingsRow>

        <SettingsRow title="Remember floating controls position" htmlFor="remember-floating-position">
          <Switch id="remember-floating-position" checked={preferences.remember_floating_position} disabled={saving}
            onCheckedChange={enabled => void savePreferences({ ...preferences, remember_floating_position: enabled })} />
        </SettingsRow>

        <SettingsRow
          title="Show conversation metrics"
          htmlFor="show-conversation-metrics"
          description="Show talk time, pace and questions during live transcription"
        >
          <Switch id="show-conversation-metrics" checked={showConversationMetrics} disabled={savingMetrics} onCheckedChange={handleMetricsToggle} />
        </SettingsRow>

        <SettingsRow
          title="Recording start reminder"
          htmlFor="recording-start-reminder"
          description="Reminds you to tell participants that the meeting is recorded"
        >
          <Switch id="recording-start-reminder" checked={showRecordingNotification} onCheckedChange={handleNotificationToggle} />
        </SettingsRow>
      </SettingsCard>

      <AudioDevicesCard
        selectedDevices={selectedDevices}
          onDeviceChange={saveSelectedDevices}
          disabled={saving || isSavingDevicePreferences || recordingBusy}
        microphoneLevel={microphoneLevel}
      />

      <CallDetectionSettings />
    </SettingsColumns>
  );
}
