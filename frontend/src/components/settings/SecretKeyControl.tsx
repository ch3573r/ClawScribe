"use client";

import { useState, type ReactNode } from "react";
import { Eye, EyeOff, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from "@/components/ui/dialog";
import { SettingsRow, StatusDot } from "@/components/ui/settings-layout";
import { cn } from "@/lib/utils";
import { describeSecret, keyProblem, type SecretStatus } from "@/lib/secret-key";
import { useCommitField, type CommitResult } from "@/components/settings/commit-field";

interface SecretKeyBase {
  id: string;
  /** What the core reports for this secret. A stored secret is shown as stored, never as text. */
  status: SecretStatus | null | undefined;
  /** Removes the stored secret. Omit when the secret cannot be removed. */
  onRemove?: () => Promise<void>;
  /** When set, removing first asks the user to confirm, with this text. */
  removeWarning?: string | null;
  placeholder?: string;
  /** Short name used in labels, for example "API key". */
  label?: string;
  disabled?: boolean;
  /**
   * A message when the secret has to be entered again, for example because the
   * address it belongs to is being changed. The field is then shown even though
   * a secret is stored, and Cancel is not offered.
   */
  required?: string | null;
}

interface CommitProps extends SecretKeyBase {
  /** Saves a newly typed secret (on Enter or when the field loses focus). */
  onCommit: (value: string) => Promise<CommitResult>;
  draft?: undefined;
}

interface DraftProps extends SecretKeyBase {
  /** A secret typed into a form that is saved later. Empty keeps the stored secret. */
  draft: string;
  onDraftChange: (value: string) => void;
  onCommit?: undefined;
}

/**
 * A secret field that never shows a stored secret. A stored one appears as
 * "Saved, ends in 1234" with Replace and Remove. The text field appears only
 * to enter a new secret, and what is typed is sent only when it was typed.
 */
export function SecretKeyControl(props: CommitProps | DraftProps) {
  const { id, status, onRemove, removeWarning, placeholder, label = "API key", disabled, required } = props;
  const stored = !!status?.stored;
  const [replacing, setReplacing] = useState(false);
  const [shown, setShown] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [removeError, setRemoveError] = useState("");

  const commitField = useCommitField({
    value: "",
    validate: (value) => keyProblem(value),
    onCommit: async (value) => {
      const result = props.onCommit ? await props.onCommit(value) : undefined;
      if (result !== "held") setReplacing(false);
      return result;
    },
    clearAfterSave: true,
  });

  const typed = props.draft !== undefined ? props.draft : commitField.draft;
  const entering = !stored || replacing || typed.length > 0 || !!required;
  const typedProblem = props.draft !== undefined ? keyProblem(props.draft) : null;

  const clearDraft = () => {
    commitField.change("");
    if (props.draft !== undefined) props.onDraftChange("");
  };

  const remove = async () => {
    if (!onRemove) return;
    setRemoving(true);
    setRemoveError("");
    try {
      await onRemove();
      setConfirming(false);
      setReplacing(false);
      clearDraft();
    } catch {
      setRemoveError(`Could not remove the ${label.toLowerCase()}. Try again`);
    } finally {
      setRemoving(false);
    }
  };

  const askRemove = () => { if (removeWarning) setConfirming(true); else void remove(); };

  const inputProps = props.draft !== undefined
    ? { value: props.draft, onChange: (event: { target: { value: string } }) => props.onDraftChange(event.target.value) }
    : commitField.inputProps;
  const { readOnly, ...rest } = inputProps as { readOnly?: boolean; value: string; onChange: (event: { target: { value: string } }) => void };

  return (
    <div className="flex flex-col items-end gap-1.5">
      <div className="flex items-center gap-2">
        {commitField.saving && <Loader2 aria-hidden="true" className="h-3.5 w-3.5 animate-spin text-muted-foreground" />}
        {entering ? (
          <>
            <div className="relative">
              <Input
                id={id}
                {...rest}
                readOnly={readOnly}
                disabled={disabled}
                type={shown ? "text" : "password"}
                autoComplete="off"
                spellCheck={false}
                placeholder={placeholder ?? `Enter your ${label.toLowerCase()}`}
                aria-invalid={commitField.error ? true : undefined}
                className={cn("w-72 max-w-full pr-10", commitField.error && "border-destructive")}
              />
              <Button type="button" variant="ghost" size="icon" className="absolute inset-y-0 right-0 h-9 w-9" aria-label={shown ? `Hide ${label.toLowerCase()}` : `Show ${label.toLowerCase()}`} aria-pressed={shown} onClick={() => setShown(!shown)}>
                {shown ? <EyeOff aria-hidden="true" /> : <Eye aria-hidden="true" />}
              </Button>
            </div>
            {stored && !required && (
              <Button type="button" variant="ghost" disabled={disabled} onClick={() => { setReplacing(false); clearDraft(); }}>
                Cancel
              </Button>
            )}
          </>
        ) : (
          <>
            <span id={id} className="inline-flex items-center gap-2 text-sm text-foreground">
              <StatusDot tone="ok" />{describeSecret(status)}
            </span>
            <Button type="button" variant="outline" disabled={disabled} onClick={() => setReplacing(true)} aria-label={`Replace ${label.toLowerCase()}`}>Replace</Button>
            {onRemove && (
              <Button type="button" variant="ghost" disabled={disabled || removing} onClick={askRemove} aria-label={`Remove ${label.toLowerCase()}`}>Remove</Button>
            )}
          </>
        )}
      </div>
      {required && !commitField.error && typedProblem === null && (
        <span role="status" className="text-[13px] text-warning-foreground">{required}</span>
      )}
      {(commitField.error || removeError || typedProblem) && <span role="alert" className="text-[13px] text-error-foreground">{commitField.error || removeError || typedProblem}</span>}
      <Dialog open={confirming} onOpenChange={(open) => { if (!removing) setConfirming(open); }}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Remove the {label.toLowerCase()}?</DialogTitle>
            <DialogDescription>{removeWarning}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="outline" disabled={removing} onClick={() => setConfirming(false)}>Keep it</Button>
            <Button type="button" variant="destructive" disabled={removing} onClick={() => void remove()}>
              {removing && <Loader2 aria-hidden="true" className="animate-spin" />}Remove
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

/** A settings row holding a [`SecretKeyControl`]. */
export function SecretKeyRow({ title, description, ...control }: { title: string; description?: ReactNode } & (Omit<CommitProps, "label"> | Omit<DraftProps, "label">)) {
  return (
    <SettingsRow title={title} htmlFor={control.id} description={description}>
      <SecretKeyControl {...(control as CommitProps | DraftProps)} label={title} />
    </SettingsRow>
  );
}
