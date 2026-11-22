'use client';

import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { getAudioDeviceDisplayName, type AudioDevice } from '@/lib/audioDevicePreferences';

const SYSTEM_DEFAULT = 'default';

export function AudioDeviceSelect({ id, label, defaultLabel, value, options, disabled, onChange, className }: {
  id: string;
  label: string;
  defaultLabel: string;
  value: string | null;
  options: AudioDevice[];
  disabled?: boolean;
  onChange: (value: string | null) => void;
  className?: string;
}) {
  const selectedIsUnavailable = value && !options.some(device => deviceValue(device) === value);
  return (
    <Select value={value ?? SYSTEM_DEFAULT} disabled={disabled}
      onValueChange={next => onChange(next === SYSTEM_DEFAULT ? null : next)}>
      <SelectTrigger id={id} aria-label={label} className={className}><SelectValue /></SelectTrigger>
      <SelectContent>
        <SelectItem value={SYSTEM_DEFAULT}>{defaultLabel}</SelectItem>
        {selectedIsUnavailable && <SelectItem value={value} disabled>{getAudioDeviceDisplayName(value, label)} (unavailable)</SelectItem>}
        {options.map(device => <SelectItem key={deviceValue(device)} value={deviceValue(device)}>{device.name}</SelectItem>)}
      </SelectContent>
    </Select>
  );
}

function deviceValue(device: AudioDevice) {
  return `${device.name} (${device.device_type.toLowerCase()})`;
}
