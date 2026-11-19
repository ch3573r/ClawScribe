"use client";

import { useState, type ReactNode } from "react";
import { Eye, EyeOff, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { PageSection } from "@/components/ui/page-section";
import { SettingsRow, StatusDot } from "@/components/ui/settings-layout";
import { cn } from "@/lib/utils";
import { useCommitField, type CommitResult, type FeedbackState } from "@/components/settings/commit-field";
import { describeProviderData } from "@/components/settings/provider-data";

/** Spinner while saving, "Saved" right after. Errors are shown with the field, see `FieldError`. */
export function SaveMark({ status }: { status?: "idle" | "saving" | "saved" | "error" }) {
  if (status === "saving") {
    return <span role="status" className="inline-flex items-center gap-1.5 text-xs text-muted-foreground"><Loader2 aria-hidden="true" className="h-3.5 w-3.5 animate-spin" />Saving…</span>;
  }
  if (status === "saved") return <span role="status" className="text-xs text-muted-foreground">Saved</span>;
  return null;
}

/** Description of a row with the field error appended, so the error sits next to the field. */
export function withError(description: ReactNode, error?: string | null): ReactNode {
  if (!error) return description;
  return <>{description}<span role="alert" className="mt-1 block text-error-foreground">{error}</span></>;
}

export const feedbackStatus = (state?: FeedbackState) => state?.status;

/** One setting with a text field that saves on blur or Enter, never on every keystroke. */
export function CommitInputRow({
  id, title, description, value, onCommit, validate, type = "text", placeholder, secret, clearAfterSave, maxLength, inputMode, className, disabled,
}: {
  id: string; title: ReactNode; description?: ReactNode; value: string;
  onCommit: (value: string) => Promise<CommitResult>; validate?: (value: string) => string | null;
  type?: "text" | "url" | "number"; placeholder?: string; secret?: boolean; clearAfterSave?: boolean;
  maxLength?: number; inputMode?: "numeric" | "decimal" | "url" | "text"; className?: string; disabled?: boolean;
}) {
  const field = useCommitField({ value, onCommit, validate, clearAfterSave });
  const [shown, setShown] = useState(false);
  const { readOnly, ...inputProps } = field.inputProps;
  return (
    <SettingsRow title={title} htmlFor={id} description={withError(description, field.error)}>
      <SaveMark status={field.status} />
      <div className="relative">
        <Input
          id={id}
          {...inputProps}
          readOnly={readOnly}
          disabled={disabled}
          type={secret && !shown ? "password" : type === "number" ? "text" : secret ? "text" : type}
          inputMode={inputMode ?? (type === "number" ? "decimal" : undefined)}
          autoComplete="off"
          spellCheck={false}
          maxLength={maxLength}
          placeholder={placeholder}
          className={cn("w-72 max-w-full", secret && "pr-10", field.error && "border-destructive", className)}
        />
        {secret && (
          <Button type="button" variant="ghost" size="icon" className="absolute inset-y-0 right-0 h-9 w-9" aria-label={shown ? `Hide ${typeof title === "string" ? title.toLowerCase() : "value"}` : `Show ${typeof title === "string" ? title.toLowerCase() : "value"}`} aria-pressed={shown} onClick={() => setShown(!shown)}>
            {shown ? <EyeOff aria-hidden="true" /> : <Eye aria-hidden="true" />}
          </Button>
        )}
      </div>
    </SettingsRow>
  );
}

/** A command the user runs in a terminal, with a copy button. */
export function CopyCommandRow({ id, title, description, command }: { id: string; title: string; description?: ReactNode; command: string }) {
  const [feedback, setFeedback] = useState("");
  const copy = async () => {
    try { await globalThis.navigator.clipboard.writeText(command); setFeedback("Copied"); }
    catch { setFeedback("Select the command and copy it manually"); }
  };
  return (
    <SettingsRow title={title} htmlFor={id} description={<>{description}{feedback && <span role="status" className="mt-1 block">{feedback}</span>}</>}>
      <Input id={id} value={command} readOnly className="w-72 max-w-full font-mono text-sm" />
      <Button type="button" variant="outline" onClick={copy} aria-label={`Copy ${title.toLowerCase()}`}>Copy</Button>
    </SettingsRow>
  );
}

/** "What this sends" for the selected summary provider. */
export function ProviderDataAside({ provider, ollamaEndpoint }: { provider: string; ollamaEndpoint?: string | null }) {
  const data = describeProviderData(provider, ollamaEndpoint);
  return (
    <PageSection title="What this sends" description={`With ${data.label} as the provider`}>
      <ul className="grid gap-4 p-5">
        {data.items.map(item => (
          <li key={item.title} className="flex items-start gap-3">
            <span className="mt-[7px]"><StatusDot tone={item.tone === "active" ? "active" : "neutral"} /></span>
            <span className="min-w-0">
              <span className="block text-sm text-foreground">{item.title}</span>
              <span className="block text-[13px] leading-5 text-muted-foreground">{item.detail}</span>
            </span>
          </li>
        ))}
      </ul>
      <div className="border-t border-border px-5 py-3">
        <Button
          type="button"
          variant="link"
          className="h-auto p-0 text-[13px]"
          onClick={() => window.dispatchEvent(new CustomEvent("open-settings-tab", { detail: "privacy" }))}
        >
          See everything in Privacy
        </Button>
      </div>
    </PageSection>
  );
}
