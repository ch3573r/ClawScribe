import React from 'react';
import { Lock, Sparkles, Cpu } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { OnboardingContainer } from '../OnboardingContainer';
import { useOnboarding } from '@/contexts/OnboardingContext';
import { usePlatform } from '@/hooks/usePlatform';
import { useConfig } from '@/contexts/ConfigContext';
import { SignalCheck } from '@/components/settings/SignalCheck';
import { useSignalCheck } from '@/components/settings/useSignalCheck';

export function WelcomeStep({ onRecordAudio, finishing, error }: {
  onRecordAudio: () => void;
  finishing: boolean;
  error: string | null;
}) {
  const { goNext } = useOnboarding();
  const windows = usePlatform() === 'windows';
  const { selectedDevices, devicePreferencesStatus } = useConfig();
  const feed = useSignalCheck(windows && devicePreferencesStatus === 'ready' && !finishing,
    selectedDevices.micDevice, selectedDevices.systemDevice);

  const features = [
    {
      icon: Lock,
      title: 'Local by default; cloud features are your choice',
    },
    {
      icon: Sparkles,
      title: 'Intelligent summaries & insights',
    },
    {
      icon: Cpu,
      title: 'Works offline, no cloud required',
    },
  ];

  return (
    <OnboardingContainer
      title="Welcome to ClawScribe"
      description="Save your meetings on this device."
      step={1}
    >
      <div className="flex flex-col items-center space-y-10">
        {/* Divider */}
        <div className="w-16 h-px bg-secondary" />

        {/* Features Card */}
        <div className="w-full max-w-md bg-card rounded-lg border border-border shadow-sm p-6 space-y-4">
          {features.map((feature, index) => {
            const Icon = feature.icon;
            return (
              <div key={index} className="flex items-start gap-3">
                <div className="flex-shrink-0 mt-0.5">
                  <div className="w-5 h-5 rounded-full bg-muted flex items-center justify-center">
                    <Icon className="w-3 h-3 text-muted-foreground" />
                  </div>
                </div>
                <p className="text-sm text-muted-foreground leading-relaxed">{feature.title}</p>
              </div>
            );
          })}
        </div>

        {windows && <div className="w-full max-w-md">
          <SignalCheck feed={feed} unavailableReason={devicePreferencesStatus === 'ready'
            ? finishing ? 'Finishing setup' : null
            : 'Audio devices are unavailable'} />
        </div>}

        {/* CTA Section */}
        <div className="w-full max-w-xs space-y-3">
          <Button
            onClick={onRecordAudio}
            disabled={finishing}
            className="w-full h-11"
          >
            {finishing ? 'Finishing setup…' : 'Record audio now'}
          </Button>
          <Button variant="outline" onClick={goNext} disabled={finishing} className="w-full h-11">
            Set up transcription and notes
          </Button>
          <p className="text-xs text-center text-muted-foreground">You can start with audio and set up transcription and notes later.</p>
          {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        </div>
      </div>
    </OnboardingContainer>
  );
}
