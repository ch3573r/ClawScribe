import React from 'react';
import { Button } from '@/components/ui/button';
import { AlertTriangle, Mic, Speaker, RefreshCw } from 'lucide-react';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { invoke } from '@tauri-apps/api/core';
import { useIsLinux } from '@/hooks/usePlatform';

interface PermissionWarningProps {
  hasMicrophone: boolean;
  hasSystemAudio: boolean;
  onRecheck: () => void;
  isRechecking?: boolean;
}

export function PermissionWarning({
  hasMicrophone,
  hasSystemAudio,
  onRecheck,
  isRechecking = false
}: PermissionWarningProps) {
  const isLinux = useIsLinux();

  // Don't show on Linux - permission handling is not needed
  if (isLinux) {
    return null;
  }

  // Don't show if both permissions are granted
  if (hasMicrophone && hasSystemAudio) {
    return null;
  }

  const isMacOS = navigator.userAgent.includes('Mac');

  const openMicrophoneSettings = async () => {
    if (isMacOS) {
      try {
        await invoke('open_system_settings', { preferencePane: 'Privacy_Microphone' });
      } catch (error) {
        console.error('Failed to open microphone settings:', error);
      }
    }
  };

  const openScreenRecordingSettings = async () => {
    if (isMacOS) {
      try {
        await invoke('open_system_settings', { preferencePane: 'Privacy_ScreenCapture' });
      } catch (error) {
        console.error('Failed to open screen recording settings:', error);
      }
    }
  };

  return (
    <div className="max-w-md mb-4 space-y-3">
      {/* Combined Permission Warning - Show when either permission is missing */}
      {(!hasMicrophone || !hasSystemAudio) && (
        <Alert variant="destructive" className="border-[hsl(var(--theme-warning-fg))]/30 bg-[hsl(var(--theme-warning-bg))]">
          <AlertTriangle className="h-5 w-5 text-[hsl(var(--theme-warning-fg))]" />
          <AlertTitle className="text-[hsl(var(--theme-warning-fg))] font-semibold">
            <div className="flex items-center gap-2">
              {!hasMicrophone && <Mic className="h-4 w-4" />}
              {!hasSystemAudio && <Speaker className="h-4 w-4" />}
              {!hasMicrophone && !hasSystemAudio ? 'Permissions required' : !hasMicrophone ? 'Microphone permission required' : 'System audio permission required'}
            </div>
          </AlertTitle>
          {/* Action Buttons */}
          <div className="mt-4 flex flex-wrap gap-2">
            {isMacOS && !hasMicrophone && (
              <Button
                variant="ghost"
                onClick={openMicrophoneSettings}
                className="h-auto whitespace-normal inline-flex items-center gap-2 px-4 py-2 text-sm font-medium text-[hsl(var(--theme-warning-fg))] bg-[hsl(var(--theme-warning-bg))] hover:bg-[hsl(var(--theme-warning-bg))]/80 rounded-md transition-colors hover:text-[hsl(var(--theme-warning-fg))]"
              >
                <Mic className="h-4 w-4" />
                Open microphone settings
              </Button>
            )}
            {isMacOS && !hasSystemAudio && (
              <Button
                variant="ghost"
                onClick={openScreenRecordingSettings}
                className="h-auto whitespace-normal inline-flex items-center gap-2 px-4 py-2 text-sm font-medium text-primary-foreground bg-primary hover:bg-primary rounded-md transition-colors hover:text-primary-foreground"
              >
                <Speaker className="h-4 w-4" />
                Open screen recording settings
              </Button>
            )}
            <Button
              variant="ghost"
              onClick={onRecheck}
              disabled={isRechecking}
              className="h-auto whitespace-normal inline-flex items-center gap-2 px-4 py-2 text-sm font-medium text-[hsl(var(--theme-warning-fg))] bg-[hsl(var(--theme-warning-bg))] hover:bg-[hsl(var(--theme-warning-bg))]/80 rounded-md transition-colors disabled:opacity-50 hover:text-[hsl(var(--theme-warning-fg))]"
            >
              <RefreshCw className={`h-4 w-4 ${isRechecking ? 'animate-spin' : ''}`} />
              Recheck
            </Button>
          </div>
          <AlertDescription className="text-[hsl(var(--theme-warning-fg))] mt-2">
            {/* Microphone Warning */}
            {!hasMicrophone && (
              <>
                <p className="mb-3">
                  ClawScribe needs access to your microphone to record meetings. No microphone devices were detected.
                </p>
                <div className="space-y-2 text-sm mb-4">
                  <p className="font-medium">Please check:</p>
                  <ul className="list-disc list-inside ml-2 space-y-1">
                    <li>Your microphone is connected and powered on</li>
                    <li>Microphone permission is granted in System Settings</li>
                    <li>No other app is exclusively using the microphone</li>
                  </ul>
                </div>
              </>
            )}

            {/* System Audio Warning */}
            {!hasSystemAudio && (
              <>
                <p className="mb-3">
                  {hasMicrophone
                    ? 'System audio capture is not available. You can still record with your microphone, but computer audio won\'t be captured.'
                    : 'System audio capture is also not available.'}
                </p>
                {isMacOS && (
                  <div className="space-y-2 text-sm mb-4">
                    <p className="font-medium">To enable system audio on macOS:</p>
                    <ul className="list-disc list-inside ml-2 space-y-1">
                      <li>Install a virtual audio device (e.g., BlackHole 2ch)</li>
                      <li>Grant Screen Recording permission to ClawScribe</li>
                      <li>Configure your audio routing in Audio MIDI Setup</li>
                    </ul>
                  </div>
                )}
              </>
            )}


          </AlertDescription>
        </Alert>
      )}
    </div>
  );
}
