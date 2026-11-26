'use client';

import { createContext, useContext, useState, type ReactNode } from 'react';
import type { PermissionStatus, OnboardingPermissions } from '@/types/onboarding';

interface OnboardingContextType {
  currentStep: 1 | 2;
  goNext: () => void;
  permissions: OnboardingPermissions;
  setPermissionStatus: (permission: keyof OnboardingPermissions, status: PermissionStatus) => void;
}

const OnboardingContext = createContext<OnboardingContextType | null>(null);

export function OnboardingProvider({ children }: { children: ReactNode }) {
  const [currentStep, setCurrentStep] = useState<1 | 2>(1);
  const [permissions, setPermissions] = useState<OnboardingPermissions>({
    microphone: 'not_determined',
    systemAudio: 'not_determined',
    screenRecording: 'not_determined',
  });
  return (
    <OnboardingContext.Provider value={{
      currentStep,
      goNext: () => setCurrentStep(2),
      permissions,
      setPermissionStatus: (permission, status) => {
        setPermissions(previous => ({ ...previous, [permission]: status }));
      },
    }}>
      {children}
    </OnboardingContext.Provider>
  );
}

export function useOnboarding() {
  const context = useContext(OnboardingContext);
  if (!context) throw new Error('useOnboarding must be used within OnboardingProvider');
  return context;
}
