import React from 'react';
import { cn } from '@/lib/utils';
import type { OnboardingContainerProps } from '@/types/onboarding';

export function OnboardingContainer({ title, description, children, step, className }: OnboardingContainerProps) {
  return (
    <div className="fixed inset-x-0 bottom-0 top-[var(--titlebar-height)] bg-muted flex items-center justify-center z-50 overflow-hidden">
      <div className={cn('w-full max-w-2xl h-full max-h-screen flex flex-col px-6 py-6', className)}>
        <div className="mb-4 text-center space-y-3 flex-shrink-0">
          {step && <p className="text-sm text-muted-foreground">Step {step} of 2</p>}
          <h1 className="text-4xl font-semibold text-foreground animate-fade-in-up">{title}</h1>
          {description && (
            <p className="text-base text-muted-foreground max-w-md mx-auto animate-fade-in-up delay-75">
              {description}
            </p>
          )}
        </div>
        <div className="flex-1 overflow-y-auto pr-2">
          <div className="space-y-6">{children}</div>
        </div>
      </div>
    </div>
  );
}
