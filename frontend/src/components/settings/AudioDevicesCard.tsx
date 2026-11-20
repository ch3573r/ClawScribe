"use client";

import { useEffect, type ReactNode } from "react";
import { useConfig } from "@/contexts/ConfigContext";
import { RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { SettingsCard, SettingsRow } from "@/components/ui/settings-layout";
import { AudioBackendSelector } from "@/components/AudioBackendSelector";
import { SegmentedLevelMeter } from "@/components/AudioLevelMeter";
import { getDeviceMetadata, type AudioDevice } from "@/components/DeviceSelection";
import Analytics from "@/lib/analytics";
import type { SelectedDevices } from "@/lib/audioDevicePreferences";
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
  placeholder: string;
  emptyText: string;
  value: string | null;
  options: AudioDevice[];
  disabled: boolean;
  onChange: (deviceName: string) => void;
  meter?: ReactNode;
};

function DeviceRow({ id, title, defaultLabel, placeholder, emptyText, value, options, disabled, onChange, meter }: DeviceRowProps) {
  const known = !value || options.some(device => value === `${device.name} (${device.device_type.toLowerCase()})`);
  return (
    <SettingsRow title={title} htmlFor={id} description={options.length === 0 ? emptyText : undefined}>
      {meter}
      <Select value={value || "default"} onValueChange={onChange} disabled={disabled}>
        <SelectTrigger id={id} className="w-96 max-w-full">
          <SelectValue placeholder={placeholder} />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="default">{defaultLabel}</SelectItem>
          {!known && value && <SelectItem value={value}>{value.replace(/\s+\((input|output)\)$/i, "")} (unavailable)</SelectItem>}
          {options.map(device => (
            <SelectItem key={device.name} value={`${device.name} (${device.device_type.toLowerCase()})`}>{device.name}</SelectItem>
          ))}
        </SelectContent>
      </Select>
    </SettingsRow>
  );
}

/** Microphone and system audio devices used for new recordings. */
export function AudioDevicesCard({ selectedDevices, onDeviceChange, disabled = false, feed, meterPaused = false }: AudioDevicesCardProps) {
  const { audioDeviceInventory } = useConfig();
  const { data, status, refresh: reload } = audioDeviceInventory;
  useEffect(() => { void reload(); }, [reload]);
  const devices = data?.devices ?? [];
  const loading = status === "checking";

  const chooseMicrophone = (deviceName: string) => {
    onDeviceChange({ ...selectedDevices, micDevice: deviceName === "default" ? null : deviceName });
    const metadata = getDeviceMetadata(deviceName);
    Analytics.track("microphone_selected", {
      device_category: metadata.category,
      is_bluetooth: metadata.isBluetooth.toString(),
      has_system_audio: (!!selectedDevices.systemDevice).toString(),
    }).catch(problem => console.error("Failed to track microphone selection:", problem));
  };

  const chooseSystemAudio = (deviceName: string) => {
    onDeviceChange({ ...selectedDevices, systemDevice: deviceName === "default" ? null : deviceName });
    const metadata = getDeviceMetadata(deviceName);
    Analytics.track("system_audio_selected", {
      device_category: metadata.category,
      is_bluetooth: metadata.isBluetooth.toString(),
      has_microphone: (!!selectedDevices.micDevice).toString(),
    }).catch(problem => console.error("Failed to track system audio selection:", problem));
  };

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
      ) : status === "error" ? (
        <SettingsRow title="Audio devices unavailable" description="Could not check audio devices. Check the system sound settings and try again.">
          <Button variant="outline" size="sm" onClick={reload} disabled={loading}>Try again</Button>
        </SettingsRow>
      ) : (
        <>
          <DeviceRow
            id="mic-selection" title="Microphone" defaultLabel="Default microphone" placeholder="Select microphone"
            emptyText="No microphone devices found"
            value={selectedDevices.micDevice} options={devices.filter(device => device.device_type === "Input")}
            disabled={disabled} onChange={chooseMicrophone}
            meter={<SegmentedLevelMeter size="mini" label="Microphone level" db={meterDb} peakDb={meterPeak} />}
          />
          <DeviceRow
            id="system-selection" title="System audio" defaultLabel="Default system audio" placeholder="Select system audio"
            emptyText="No system audio devices found"
            value={selectedDevices.systemDevice} options={devices.filter(device => device.device_type === "Output")}
            disabled={disabled} onChange={chooseSystemAudio}
          />
          {!disabled && <div className="px-5 py-3 empty:hidden"><AudioBackendSelector disabled={disabled} /></div>}
        </>
      )}
    </SettingsCard>
  );
}
