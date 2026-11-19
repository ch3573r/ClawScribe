import type { HTMLAttributes, ReactNode } from "react";
import { cn } from "@/lib/utils";

export function PageSection({ icon, title, description, actions, children, className, ...props }: Omit<HTMLAttributes<HTMLElement>, "title"> & {
  icon?: ReactNode; title?: ReactNode; description?: ReactNode; actions?: ReactNode;
}) {
  return <section className={cn("rounded-lg border border-border bg-card text-foreground", className)} {...props}>
    {title && <header className="flex items-start justify-between gap-3 border-b border-border p-5">
      <div className="flex min-w-0 items-start gap-3">
        {icon && <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-border bg-background">{icon}</span>}
        <div className="min-w-0"><h2 className="font-semibold">{title}</h2>{description && <p className="mt-1 text-sm text-muted-foreground">{description}</p>}</div>
      </div>
      {actions}
    </header>}
    {children}
  </section>;
}
