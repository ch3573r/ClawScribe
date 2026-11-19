"use client";

import type { ReactNode, RefObject } from "react";
import { AlertTriangle, Loader2, RefreshCw } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

/**
 * Small pieces shared by the Connections and Diagnostics pages. They sit on top
 * of the settings-layout primitives and add no new look of their own.
 */

/** Width of a select that sits at the right of a row. */
export const ROW_SELECT = "w-60 max-w-full";

/** Two cards side by side on wide windows, one column when narrow. */
export const TWO_COLUMNS = "grid gap-5 min-[1400px]:grid-cols-2";

/** 36 px tile that holds a product icon. */
export function IconTile({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <span className={cn("flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-border bg-background text-muted-foreground", className)}>
      {children}
    </span>
  );
}

/** Ghost icon button for a card header. Spins while `loading`. */
export function IconAction({ label, onClick, disabled, loading, icon }: {
  label: string; onClick: () => void; disabled?: boolean; loading?: boolean; icon?: ReactNode;
}) {
  return (
    <Button type="button" variant="ghost" size="icon" aria-label={label} title={label} onClick={onClick} disabled={disabled || loading}>
      {icon ?? <RefreshCw aria-hidden="true" className={loading ? "animate-spin" : ""} />}
    </Button>
  );
}

/** A full-width line inside a card for an error, a warning or a hint. */
export function NoteRow({ tone = "muted", children, className, role }: {
  tone?: "muted" | "warning" | "error"; children: ReactNode; className?: string; role?: "alert" | "status";
}) {
  const alerting = tone !== "muted";
  return (
    <div
      role={role ?? (tone === "error" ? "alert" : undefined)}
      className={cn(
        "flex items-start gap-2.5 px-5 py-3 text-[13px] leading-5",
        tone === "warning" && "bg-warning text-warning-foreground",
        tone === "error" && "text-error-foreground",
        tone === "muted" && "text-muted-foreground",
        className,
      )}
    >
      {alerting && <AlertTriangle aria-hidden="true" className="mt-0.5 h-4 w-4 shrink-0" />}
      <div className="min-w-0 flex-1 break-words">{children}</div>
    </div>
  );
}

/** Inline "create new" form that appears under a select. Enter submits, Escape cancels. */
export function CreateRow({ label, value, onChange, onSubmit, onCancel, placeholder, maxLength, saving, hint, inputRef }: {
  label: string; value: string; onChange: (value: string) => void; onSubmit: () => void; onCancel: () => void;
  placeholder: string; maxLength: number; saving: boolean; hint?: string;
  inputRef?: RefObject<HTMLInputElement | null>;
}) {
  return (
    <div className="grid gap-2 px-5 py-3">
      <label className="text-sm text-foreground">{label}</label>
      <div className="flex flex-wrap items-center gap-2">
        <Input
          type="text"
          autoFocus
          ref={inputRef}
          aria-label={label}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") onSubmit();
            if (e.key === "Escape") onCancel();
          }}
          placeholder={placeholder}
          maxLength={maxLength}
          disabled={saving}
          className="min-w-0 flex-1 basis-48"
        />
        <Button type="button" size="sm" onClick={onSubmit} disabled={saving || !value.trim()}>
          {saving ? <Loader2 aria-hidden="true" className="animate-spin" /> : null}
          Create
        </Button>
        <Button type="button" size="sm" variant="outline" onClick={onCancel} disabled={saving}>Cancel</Button>
      </div>
      {hint && <p className="text-xs text-muted-foreground">{hint}</p>}
    </div>
  );
}
