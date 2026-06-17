import React from 'react';
import Image from 'next/image';
import { Dialog, DialogContent, DialogTitle, DialogTrigger } from './ui/dialog';
import { VisuallyHidden } from './ui/visually-hidden';
import { About } from './About';

interface LogoProps {
  isCollapsed: boolean;
}

const Logo = React.forwardRef<HTMLButtonElement, LogoProps>(({ isCollapsed }, ref) => {
  return (
    <Dialog aria-describedby={undefined}>
      <DialogTrigger asChild>
        <button
          ref={ref}
          className={`flex cursor-pointer items-center border-none bg-transparent p-0 text-left transition-opacity hover:opacity-85 ${
            isCollapsed ? 'justify-center' : 'gap-3'
          }`}
        >
          <Image
            src="/brand/clawscribe-icon-64.png"
            alt="ClawScribe"
            width={isCollapsed ? 34 : 38}
            height={isCollapsed ? 34 : 38}
            className="rounded-xl"
          />
          {!isCollapsed && (
            <span className="flex flex-col leading-none">
              <span className="text-lg font-semibold tracking-tight text-foreground">ClawScribe</span>
              <span className="mt-1 text-xs font-medium uppercase tracking-[0.22em] text-primary/70">
                Meeting AI
              </span>
            </span>
          )}
        </button>
      </DialogTrigger>
      <DialogContent>
        <VisuallyHidden>
          <DialogTitle>About ClawScribe</DialogTitle>
        </VisuallyHidden>
        <About />
      </DialogContent>
    </Dialog>
  );
});

Logo.displayName = 'Logo';

export default Logo;
