"use client";

import { forwardRef, type ComponentPropsWithoutRef, type ElementRef } from "react";
import { Slider as SliderPrimitive } from "radix-ui";
import { cn } from "@/lib/utils";

type SliderProps = ComponentPropsWithoutRef<typeof SliderPrimitive.Root> & {
  "aria-label": string;
  "aria-valuetext"?: string;
};

const Slider = forwardRef<ElementRef<typeof SliderPrimitive.Root>, SliderProps>(
  ({ className, "aria-label": label, "aria-valuetext": valueText, ...props }, ref) => (
    <SliderPrimitive.Root ref={ref} className={cn("relative flex h-8 w-full touch-none select-none items-center data-[disabled]:opacity-50", className)} {...props}>
      <SliderPrimitive.Track className="relative h-1.5 w-full grow overflow-hidden rounded-full bg-muted">
        <SliderPrimitive.Range className="absolute h-full bg-primary" />
      </SliderPrimitive.Track>
      <SliderPrimitive.Thumb aria-label={label} aria-valuetext={valueText} className="block size-3 rounded-full border border-primary bg-background shadow-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background" />
    </SliderPrimitive.Root>
  ),
);
Slider.displayName = SliderPrimitive.Root.displayName;

export { Slider };
