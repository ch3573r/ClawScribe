"use client";

import { useEffect, type ReactNode } from "react";
import { useConfig } from "@/contexts/ConfigContext";
import { RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { AudioDeviceSelect } from "@/components/AudioDeviceSelect";
import { SettingsCard, SettingsRow } from "@/components/ui/settings-layout";
import { AudioBackendSelector } from "@/components/AudioBackendSelector";
import { SegmentedLevelMeter } from "@/components/AudioLevelMeter";
import type { AudioDevice, SelectedDevices } from "@/lib/audioDevicePreferences";
import type { AudioLevelFeed } from "@/components/settings/useAudioLevelFeed";

interface AudioDevicesCardProps {
  selectedDevices: SelectedDevices;
  onDeviceChange: (devices: SelectedDevices) => void;
  disabled?: boolean;
  /** Level of the microphone, drawn as a small meter in its row. */
  feed: AudioLevelFeed;
  /** Set while the level feed is paused, for example during a recording. */
  meterPaused?: boolean;
}

type DeviceRowProps = {
  id: string;
  title: string;
  defaultLabel: string;
  emptyText: string;
  value: string | null;
  options: AudioDevice[];
  disabled: boolean;
  onChange: (deviceName: string | null) => void;
  meter?: ReactNode;
};

function DeviceRow({ id, title, defaultLabel, emptyText, value, options, disabled, onChange, meter }: DeviceRowProps) {
  return (
    <SettingsRow title={title} htmlFor={id} description={options.length === 0 ? emptyText : undefined}>
      {meter}
      <AudioDeviceSelect id={id} label={title} defaultLabel={defaultLabel} value={value} options={options}
        className="w-96 max-w-full" disabled={disabled} onChange={onChange} />
    </SettingsRow>
  );
}

/** Microphone and system audio devices used for new recordings. */
export function AudioDevicesCard({ selectedDevices, onDeviceChange, disabled = false, feed, meterPaused = false }: AudioDevicesCardProps) {
  const { audioDeviceInventory, devicePreferencesStatus, refreshDevicePreferences } = useConfig();
  const { data, status, refresh } = audioDeviceInventory;
  useEffect(() => { void refresh(); }, [refresh]);
  const reload = () => {
    void refresh();
    if (devicePreferencesStatus === 'error') void refreshDevicePreferences();
  };
  const devices = data?.devices ?? [];
  const loading = status === "checking" || devicePreferencesStatus === 'loading';

  const meterDb = !meterPaused && feed.status === "live" && feed.microphone ? feed.microphone.db : null;
  const meterPeak = meterDb === null ? null : feed.microphone?.peakDb ?? null;

  return (
    <SettingsCard
      title="Audio devices"
      description="Used for new recordings"
      aria-busy={loading}
      actions={
        <Button variant="ghost" size="icon" aria-label="Recheck devices" title="Recheck devices" disabled={loading} onClick={reload}>
          <RefreshCw aria-hidden="true" />
        </Button>
      }
    >
      {loading ? (
        <SettingsRow title="Loading audio devices" />
      ) : status === "error" || devicePreferencesStatus === 'error' ? (
        <SettingsRow title="Audio devices unavailable" description="Could not check audio devices. Check the system sound settings and try again.">
          <Button variant="outline" size="sm" onClick={reload} disabled={loading}>Try again</Button>
        </SettingsRow>
      ) : (
        <>
          <DeviceRow
            id="mic-selection" title="Microphone" defaultLabel="Default microphone"
            emptyText="No microphone devices found"
            value={selectedDevices.micDevice} options={devices.filter(device => device.device_type === "Input")}
            disabled={disabled} onChange={micDevice => onDeviceChange({ ...selectedDevices, micDevice })}
            meter={<SegmentedLevelMeter size="mini" label="Microphone level" db={meterDb} peakDb={meterPeak} />}
          />
          <DeviceRow
            id="system-selection" title="System audio" defaultLabel="Default system audio"
            emptyText="No system audio devices found"
            value={selectedDevices.systemDevice} options={devices.filter(device => device.device_type === "Output")}
            disabled={disabled} onChange={systemDevice => onDeviceChange({ ...selectedDevices, systemDevice })}
          />
          {!disabled && <div className="px-5 py-3 empty:hidden"><AudioBackendSelector disabled={disabled} /></div>}
        </>
      )}
    </SettingsCard>
  );
}
