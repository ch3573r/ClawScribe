import React from 'react';
import Image from 'next/image';
import { Button } from './ui/button';
import { Dialog, DialogContent, DialogDescription, DialogTitle, DialogTrigger } from './ui/dialog';
import { Tooltip, TooltipContent, TooltipTrigger } from './ui/tooltip';
import { About } from './About';

interface LogoProps {
  isCollapsed: boolean;
}

const Logo = React.forwardRef<HTMLButtonElement, LogoProps>(({ isCollapsed }, ref) => {
  return (
    <Dialog>
      <Tooltip>
        <TooltipTrigger asChild>
          <DialogTrigger asChild>
            <Button ref={ref} type="button" variant="ghost" aria-label="About ClawScribe"
              className={`h-auto p-1 text-left ${isCollapsed ? 'justify-center' : 'gap-3'}`}>
              <Image
                src="/brand/clawscribe-icon-64.png"
                alt=""
                width={isCollapsed ? 34 : 38}
                height={isCollapsed ? 34 : 38}
                className="rounded-md"
              />
              {!isCollapsed && <span className="text-lg font-semibold tracking-tight text-sidebar-foreground">ClawScribe</span>}
            </Button>
          </DialogTrigger>
        </TooltipTrigger>
        <TooltipContent side="right">About ClawScribe</TooltipContent>
      </Tooltip>
      <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-y-auto">
        <DialogTitle>About ClawScribe</DialogTitle>
        <DialogDescription className="sr-only">Version, licensing and bundled software</DialogDescription>
        <About compact />
      </DialogContent>
    </Dialog>
  );
});

Logo.displayName = 'Logo';

export default Logo;
