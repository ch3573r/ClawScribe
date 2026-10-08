"use client";

import * as React from "react";
import { cn } from "@/lib/utils";

const MultiSelect = React.forwardRef<HTMLSelectElement, React.ComponentProps<"select">>(
  ({ className, multiple = true, ...props }, ref) => (
    <select ref={ref} multiple={multiple} className={cn(className)} {...props} />
  ),
);
MultiSelect.displayName = "MultiSelect";

export { MultiSelect };
