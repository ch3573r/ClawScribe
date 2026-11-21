import { useEffect, useRef, useState } from 'react';
import { useOnboarding } from '@/contexts/OnboardingContext';
import { finishRecordingSetup } from '@/lib/onboarding-recording';
import type { RecordingMode } from '@/contexts/RecordingStateContext';
import {
  WelcomeStep,
  PermissionsStep,
  SetupOverviewStep,
} from './steps';

interface OnboardingFlowProps {
  onComplete: () => void;
}

export function OnboardingFlow({ onComplete }: OnboardingFlowProps) {
  const { currentStep } = useOnboarding();
  const [isMac, setIsMac] = useState(false);
  const [permissionStep, setPermissionStep] = useState(false);
  const [finishing, setFinishing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const finishingRef = useRef(false);
  const choice = useRef<{ mode: RecordingMode; localNotesModel: string | null; automaticNotes: boolean } | null>(null);

  useEffect(() => {
    // Check if running on macOS
    const checkPlatform = async () => {
      try {
        // Dynamic import to avoid SSR issues if any
        const { platform } = await import('@tauri-apps/plugin-os');
        setIsMac(platform() === 'macos');
      } catch (e) {
        console.error('Failed to detect platform:', e);
        // Fallback
        setIsMac(navigator.userAgent.includes('Mac'));
      }
    };
    checkPlatform();
  }, []);

  const finish = async () => {
    if (!choice.current || finishingRef.current) return;
    finishingRef.current = true;
    setFinishing(true);
    setError(null);
    try {
      await finishRecordingSetup(choice.current.mode, choice.current.localNotesModel, choice.current.automaticNotes);
      onComplete();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'Could not finish setup. Try again.');
    } finally {
      finishingRef.current = false;
      setFinishing(false);
    }
  };

  const choose = (mode: RecordingMode, localNotesModel: string | null, automaticNotes = false) => {
    choice.current = { mode, localNotesModel, automaticNotes };
    if (isMac) setPermissionStep(true);
    else void finish();
  };

  return (
    <div className="onboarding-flow">
      {currentStep === 1 && !permissionStep &&
        <WelcomeStep onRecordAudio={() => choose('audio_only', null)} finishing={finishing} error={error} />}
      {currentStep === 2 && !permissionStep && <SetupOverviewStep onChoose={choose} finishing={finishing} error={error} />}
      {permissionStep && <PermissionsStep onFinish={finish} finishing={finishing} error={error} />}
    </div>
  );
}
