import React, { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { RefreshCw, Mic, Speaker } from 'lucide-react';
import { AudioLevelMeter, CompactAudioLevelMeter } from './AudioLevelMeter';
import { AudioBackendSelector } from './AudioBackendSelector';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { Label } from '@/components/ui/label';
import { Button } from '@/components/ui/button';
import Analytics from '@/lib/analytics';
import type { SelectedDevices } from '@/lib/audioDevicePreferences';

export type { SelectedDevices } from '@/lib/audioDevicePreferences';

export interface AudioDevice {
  name: string;
  device_type: 'Input' | 'Output';
}

export interface AudioLevelData {
  device_name: string;
  device_type: string;
  rms_level: number;
  peak_level: number;
  is_active: boolean;
}

export interface AudioLevelUpdate {
  timestamp: number;
  levels: AudioLevelData[];
}

interface MonitoredMicrophone {
  requested_name: string;
  device_name: string;
  used_default: boolean;
}

interface DeviceSelectionProps {
  selectedDevices: SelectedDevices;
  onDeviceChange: (devices: SelectedDevices) => void;
  disabled?: boolean;
}

export function DeviceSelection({ selectedDevices, onDeviceChange, disabled = false }: DeviceSelectionProps) {
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [audioLevels, setAudioLevels] = useState<Map<string, AudioLevelData>>(new Map());
  const [isMonitoring, setIsMonitoring] = useState(false);
  const [showLevels, setShowLevels] = useState(false);
  const [monitoredInputDevices, setMonitoredInputDevices] = useState<MonitoredMicrophone[]>([]);

  // Filter devices by type
  const inputDevices = devices.filter(device => device.device_type === 'Input');
  const outputDevices = devices.filter(device => device.device_type === 'Output');

  // Fetch available audio devices
  const fetchDevices = async () => {
    try {
      setError(null);
      const result = await invoke<AudioDevice[]>('get_audio_devices');
      setDevices(result);
      console.log('Fetched audio devices:', result);
    } catch (err) {
      console.error('Failed to fetch audio devices:', err);
      setError('Failed to load audio devices. Please check your system audio settings.');
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  };

  // Load devices on component mount
  useEffect(() => {
    fetchDevices();
  }, []);

  // Set up audio level event listener
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    const setupAudioLevelListener = async () => {
      try {
        unlisten = await listen<AudioLevelUpdate>('audio-levels', (event) => {
          const levelUpdate = event.payload;
          const newLevels = new Map<string, AudioLevelData>();

          levelUpdate.levels.forEach(level => {
            newLevels.set(level.device_name, level);
          });

          setAudioLevels(newLevels);
        });
      } catch (err) {
        console.error('Failed to setup audio level listener:', err);
      }
    };

    setupAudioLevelListener();

    // Cleanup function
    return () => {
      if (unlisten) {
        unlisten();
      }
      // Stop monitoring when component unmounts
      if (isMonitoring) {
        stopAudioLevelMonitoring();
      }
    };
  }, [isMonitoring]);

  // Handle device refresh
  const handleRefresh = async () => {
    setRefreshing(true);
    await fetchDevices();
  };

  // Helper function to detect device category and Bluetooth status
  const getDeviceMetadata = (deviceName: string) => {
    const nameLower = deviceName.toLowerCase();

    // Detect if it's Bluetooth
    const isBluetooth = nameLower.includes('airpods')
      || nameLower.includes('bluetooth')
      || nameLower.includes('wireless')
      || nameLower.includes('wh-')  // Sony WH-* series
      || nameLower.includes('bt ');

    // Categorize device
    let category = 'wired';
    if (deviceName === 'default') {
      category = 'default';
    } else if (nameLower.includes('airpods')) {
      category = 'airpods';
    } else if (isBluetooth) {
      category = 'bluetooth';
    }

    return { isBluetooth, category };
  };

  // Handle microphone device selection
  const handleMicDeviceChange = (deviceName: string) => {
    const newDevices = {
      ...selectedDevices,
      micDevice: deviceName === 'default' ? null : deviceName
    };
    onDeviceChange(newDevices);

    // Track device selection analytics with enhanced metadata
    const metadata = getDeviceMetadata(deviceName);
    Analytics.track('microphone_selected', {
      device_category: metadata.category,
      is_bluetooth: metadata.isBluetooth.toString(),
      has_system_audio: (!!selectedDevices.systemDevice).toString()
    }).catch(err => console.error('Failed to track microphone selection:', err));
  };

  // Handle system audio device selection
  const handleSystemDeviceChange = (deviceName: string) => {
    const newDevices = {
      ...selectedDevices,
      systemDevice: deviceName === 'default' ? null : deviceName
    };
    onDeviceChange(newDevices);

    // Track device selection analytics with enhanced metadata
    const metadata = getDeviceMetadata(deviceName);
    Analytics.track('system_audio_selected', {
      device_category: metadata.category,
      is_bluetooth: metadata.isBluetooth.toString(),
      has_microphone: (!!selectedDevices.micDevice).toString()
    }).catch(err => console.error('Failed to track system audio selection:', err));
  };

  // Start audio level monitoring
  const startAudioLevelMonitoring = async () => {
    try {
      // Only monitor input devices for now (microphones). If the user picked a
      // specific microphone, test that device; otherwise show all detected mics.
      const selectedMicName = selectedDevices.micDevice
        ? selectedDevices.micDevice.replace(/\s+\(input\)$/i, '').trim()
        : null;
      const deviceNames = selectedMicName
        ? [selectedMicName]
        : inputDevices.map(device => device.name);

      if (deviceNames.length === 0) {
        setError('No microphone devices found to monitor');
        return;
      }

      setError(null);
      const monitoredDevices = await invoke<MonitoredMicrophone[]>('start_audio_level_monitoring', { deviceNames });
      setIsMonitoring(true);
      setShowLevels(true);
      setMonitoredInputDevices(monitoredDevices);
    } catch (err) {
      console.error('Failed to start audio level monitoring:', err);
      setError(String(err));
    }
  };

  // Stop audio level monitoring
  const stopAudioLevelMonitoring = async () => {
    try {
      await invoke('stop_audio_level_monitoring');
      setIsMonitoring(false);
      setShowLevels(false);
      setAudioLevels(new Map());
      setMonitoredInputDevices([]);
      setError(null);
      console.log('Stopped audio level monitoring');
    } catch (err) {
      console.error('Failed to stop audio level monitoring:', err);
    }
  };

  // Toggle audio level monitoring
  const toggleAudioLevelMonitoring = async () => {
    if (isMonitoring) {
      await stopAudioLevelMonitoring();
    } else {
      await startAudioLevelMonitoring();
    }
  };

  if (loading) {
    return (
      <div className="p-4 space-y-4">
        <div className="animate-pulse">
          <div className="h-4 bg-secondary rounded w-1/3 mb-4"></div>
          <div className="h-10 bg-secondary rounded mb-3"></div>
          <div className="h-10 bg-secondary rounded"></div>
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h4 className="text-sm font-medium text-foreground">Audio devices</h4>
        <div className="flex items-center space-x-2">
          <Button
            variant="ghost"
            onClick={toggleAudioLevelMonitoring}
            disabled={disabled || inputDevices.length === 0}
            className={`whitespace-normal py-0 [&_svg]:size-3.5 h-8 px-3 inline-flex items-center gap-2 rounded-md text-xs font-medium transition-colors disabled:pointer-events-none disabled:opacity-50 ${
              isMonitoring
                ? 'bg-error text-error-foreground hover:bg-error-foreground/15 hover:text-error-foreground'
                : 'bg-success text-success-foreground hover:bg-success-foreground/15 hover:text-success-foreground'
            }`}
            title={inputDevices.length === 0 ? 'No microphones available to test' : isMonitoring ? 'Stop microphone test' : 'Test microphone levels'}
          >
            <Mic className="h-3.5 w-3.5" />
            {isMonitoring ? 'Stop test' : 'Test mic'}
          </Button>
          <Button
            variant="ghost"
            onClick={handleRefresh}
            disabled={refreshing || disabled}
            className="whitespace-normal gap-0 hover:text-foreground h-8 w-8 p-0 inline-flex items-center justify-center rounded-md text-sm font-medium transition-colors hover:bg-muted disabled:pointer-events-none disabled:opacity-50"
          >
            <RefreshCw className={`h-4 w-4 ${refreshing ? 'animate-spin' : ''}`} />
          </Button>
        </div>
      </div>

      {error && (
        <div className="p-3 text-sm text-error-foreground bg-error border border-error-border/25 rounded-md">
          {error}
        </div>
      )}

      <div className="space-y-3">
        {/* Microphone Selection */}
        <div className="space-y-2">
          <div className="flex items-center gap-2">
            <Mic className="h-4 w-4 text-muted-foreground" />
            <Label htmlFor="mic-selection" className="text-sm font-medium text-foreground">
              Microphone
            </Label>
          </div>
          <Select
            value={selectedDevices.micDevice || 'default'}
            onValueChange={handleMicDeviceChange}
            disabled={disabled}
          >
            <SelectTrigger id="mic-selection" className="w-full">
              <SelectValue placeholder="Select microphone" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="default">Default microphone</SelectItem>
              {inputDevices.map((device) => (
                <SelectItem
                  key={device.name}
                  value={`${device.name} (${device.device_type.toLowerCase()})`}
                >
                  {device.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {inputDevices.length === 0 && (
            <p className="text-xs text-muted-foreground">No microphone devices found</p>
          )}

          {/* Audio Level Meters for Input Devices */}
          {showLevels && monitoredInputDevices.length > 0 && (
            <div className="space-y-2 pt-2 border-t border-border">
              <p className="text-xs text-muted-foreground font-medium">
                {monitoredInputDevices.length === 1 ? 'Microphone level:' : 'Microphone levels:'}
              </p>
              {monitoredInputDevices.map((device) => {
                const levelData = audioLevels.get(device.device_name);
                return (
                  <div key={`level-${device.device_name}`} className="space-y-1">
                    <div className="flex items-center justify-between">
                      <span className="text-xs text-muted-foreground truncate max-w-[200px]">
                        {device.device_name}
                      </span>
                      {levelData && (
                        <CompactAudioLevelMeter
                          rmsLevel={levelData.rms_level}
                          peakLevel={levelData.peak_level}
                          isActive={levelData.is_active}
                        />
                      )}
                    </div>
                    {levelData && (
                      <AudioLevelMeter
                        rmsLevel={levelData.rms_level}
                        peakLevel={levelData.peak_level}
                        isActive={levelData.is_active}
                        deviceName={device.device_name}
                        size="small"
                      />
                    )}
                    {device.used_default && (
                      <p className="text-xs text-warning-foreground">
                        The preferred microphone is unavailable. Testing the Windows default microphone shown above.
                      </p>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>

        {/* System Audio Selection */}
        <div className="space-y-2">
          <div className="flex items-center gap-2">
            <Speaker className="h-4 w-4 text-muted-foreground" />
            <Label htmlFor="system-selection" className="text-sm font-medium text-foreground">
              System audio
            </Label>
          </div>

          <Select
            value={selectedDevices.systemDevice || 'default'}
            onValueChange={handleSystemDeviceChange}
            disabled={disabled}
          >
            <SelectTrigger id="system-selection" className="w-full">
              <SelectValue placeholder="Select system audio" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="default">Default system audio</SelectItem>
              {outputDevices.map((device) => (
                <SelectItem
                  key={device.name}
                  value={`${device.name} (${device.device_type.toLowerCase()})`}
                >
                  {device.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>

          {outputDevices.length === 0 && (
            <p className="text-xs text-muted-foreground">No system audio devices found</p>
          )}

          {/* Backend Selection - available on all platforms */}
          {!disabled && (
            <div className="pt-3 border-t border-border">
              <AudioBackendSelector disabled={disabled} />
            </div>
          )}
        </div>
      </div>

      {/* Info text */}
      <div className="text-xs text-muted-foreground space-y-1">
        <p>• <strong>Microphone:</strong> Records your voice and ambient sound</p>
        <p>• <strong>System audio:</strong> Records computer audio (music, calls, etc.)</p>
        {isMonitoring && (
          <p>• <strong>Mic levels:</strong> Green = good, yellow = loud, red = too loud</p>
        )}
        {!isMonitoring && inputDevices.length > 0 && (
          <p>• <strong>Tip:</strong> Click "Test mic" to check if your microphone is working</p>
        )}
      </div>
    </div>
  );
}
