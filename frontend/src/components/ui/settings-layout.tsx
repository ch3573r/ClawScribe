import type { HTMLAttributes, ReactNode } from "react";
import { cn } from "@/lib/utils";
import { PageSection } from "@/components/ui/page-section";

/**
 * Shared building blocks for every settings page.
 *
 * A page is a column of cards. Each card is a list of rows: text on the left,
 * one control on the right. State is shown as a dot with plain text, never as
 * a filled pill or a coloured tile.
 */

/** Single column, 880 px wide, aligned to the settings menu. `wide` spans both columns. */
export function SettingsStack({ wide, className, ...props }: HTMLAttributes<HTMLDivElement> & { wide?: boolean }) {
  return <div className={cn("grid min-w-0 gap-5", wide ? "max-w-[82rem]" : "max-w-[55rem]", className)} {...props} />;
}

/**
 * Main column with an aside for live feedback or context. The aside moves
 * below the main column when the window is too narrow for both.
 */
export function SettingsColumns({ aside, children, className }: { aside: ReactNode; children: ReactNode; className?: string }) {
  return (
    <div className={cn("grid items-start gap-5 min-[1700px]:grid-cols-[minmax(0,55rem)_minmax(0,25rem)] min-[1700px]:gap-8", className)}>
      <div className="grid min-w-0 max-w-[55rem] gap-5">{children}</div>
      <aside className="grid min-w-0 max-w-[55rem] gap-5">{aside}</aside>
    </div>
  );
}

/** A card of rows. Give it a title only when it differs from the page title. */
export function SettingsCard({ icon, title, description, actions, children, className, ...props }: Omit<HTMLAttributes<HTMLElement>, "title"> & {
  icon?: ReactNode; title?: ReactNode; description?: ReactNode; actions?: ReactNode;
}) {
  return (
    <PageSection icon={icon} title={title} description={description} actions={actions} className={className} {...props}>
      <div className="divide-y divide-border/60">{children}</div>
    </PageSection>
  );
}

/** One setting: title and optional description on the left, one control on the right. */
export function SettingsRow({ icon, title, description, children, className, htmlFor, ...props }: Omit<HTMLAttributes<HTMLDivElement>, "title"> & {
  icon?: ReactNode; title: ReactNode; description?: ReactNode; htmlFor?: string;
}) {
  const Title = htmlFor ? "label" : "div";
  return (
    <div className={cn("flex min-h-16 flex-wrap items-center gap-x-6 gap-y-3 px-5 py-3", className)} {...props}>
      {icon && <span className="shrink-0 text-muted-foreground [&_svg]:h-[18px] [&_svg]:w-[18px]">{icon}</span>}
      <div className="min-w-0 flex-1 basis-40">
        <Title {...(htmlFor ? { htmlFor } : {})} className="block text-sm text-foreground">{title}</Title>
        {description && <p className="mt-0.5 text-[13px] leading-5 text-muted-foreground">{description}</p>}
      </div>
      {children && <div className="flex shrink-0 items-center gap-2">{children}</div>}
    </div>
  );
}

/** Quiet last line of a card: a status, a hint, or a secondary action on the right. */
export function SettingsFooter({ actions, children, className, ...props }: HTMLAttributes<HTMLDivElement> & { actions?: ReactNode }) {
  return (
    <div className={cn("flex min-h-12 flex-wrap items-center gap-x-4 gap-y-2 px-5 py-2.5 text-[13px] text-muted-foreground", className)} {...props}>
      <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-4 gap-y-1">{children}</div>
      {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
    </div>
  );
}

export type StatusTone = "neutral" | "ok" | "active" | "warning" | "error" | "off";

const toneClasses: Record<StatusTone, string> = {
  neutral: "bg-muted-foreground/50",
  ok: "bg-success-foreground",
  active: "bg-primary",
  warning: "bg-warning-foreground",
  error: "bg-error-foreground",
  off: "bg-transparent ring-1 ring-inset ring-muted-foreground/70",
};

/** An 8 px status lamp. Decorative: always pair it with text. */
export function StatusDot({ tone = "neutral", className }: { tone?: StatusTone; className?: string }) {
  return <span aria-hidden="true" className={cn("inline-block h-2 w-2 shrink-0 rounded-full", toneClasses[tone], className)} />;
}

/** A status lamp with its text. Only warnings and errors colour the text. */
export function StatusText({ tone = "neutral", children, className }: { tone?: StatusTone; children: ReactNode; className?: string }) {
  return (
    <span className={cn(
      "inline-flex items-center gap-2 whitespace-nowrap text-sm",
      tone === "warning" ? "text-warning-foreground" : tone === "error" ? "text-error-foreground" : tone === "off" || tone === "neutral" ? "text-muted-foreground" : "text-foreground",
      className,
    )}>
      <StatusDot tone={tone} />{children}
    </span>
  );
}

/** Quiet inline tag such as "Beta" or "Recommended". */
export function SettingsTag({ children, className }: { children: ReactNode; className?: string }) {
  return <span className={cn("ml-2 inline-block rounded border border-border px-1.5 align-[2px] font-mono text-[11px] font-medium leading-[18px] text-muted-foreground", className)}>{children}</span>;
}

/** Version numbers, times, paths, sizes and counts. */
export function Meta({ children, className }: { children: ReactNode; className?: string }) {
  return <span className={cn("whitespace-nowrap font-mono text-xs tabular-nums text-muted-foreground", className)}>{children}</span>;
}

/** One keycap. Render a shortcut as several keycaps side by side. */
export function Kbd({ children, className }: { children: ReactNode; className?: string }) {
  return <kbd className={cn("inline-grid h-[26px] min-w-[30px] place-items-center rounded-md border border-b-2 border-border bg-muted px-2 font-mono text-xs font-medium text-foreground", className)}>{children}</kbd>;
}
