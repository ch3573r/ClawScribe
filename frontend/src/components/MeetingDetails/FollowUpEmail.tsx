"use client";

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";

interface Draft {
  recipients: { name: string | null; email: string }[];
  subject: string;
  body: string;
}

export type FollowUpEmailDraft = { subject?: string | null; body?: string | null };

const MAX_RECIPIENTS = 25;
const EMAIL_ADDRESS = /^[a-z0-9_+%'-]+(?:\.[a-z0-9_+%'-]+)*@(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/i;


export function FollowUpEmail({ meetingId, close, initialDraft }: {
  meetingId: string;
  close: () => void;
  initialDraft?: FollowUpEmailDraft | null;
}) {
  const [draft, setDraft] = useState<Draft | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [manualRecipients, setManualRecipients] = useState('');
  const [attempt, setAttempt] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const pending = useRef(false);
  const initial = useRef(initialDraft);
  initial.current = initialDraft;
  useEffect(() => {
    let active = true;
    setDraft(null);
    setError("");
    setSelected([]);
    setManualRecipients('');
    void invoke<Draft>("get_reviewed_follow_up_draft", { meetingId }).then(data => {
      if (active) setDraft({
        ...data, subject: initial.current?.subject || data.subject, body: initial.current?.body || data.body,
      });
    }).catch(() => { if (active) setError("Could not load the follow-up draft."); });
    return () => { active = false; };
  }, [meetingId, attempt]);

  const manual = manualRecipients.split(/[,;]/).map(value => value.trim()).filter(Boolean);
  const unique = new Map<string, string>();
  for (const email of [...selected, ...manual]) {
    const address = email.toLowerCase();
    if (!unique.has(address)) unique.set(address, email);
  }
  const recipients = [...unique.values()];
  const recipientError = manual.some(email => email.length > 254 || !EMAIL_ADDRESS.test(email))
    ? 'Enter valid email addresses separated by commas or semicolons.'
    : recipients.length > MAX_RECIPIENTS ? `Use up to ${MAX_RECIPIENTS} recipients.` : '';

  async function openDraft() {
    if (!draft?.subject.trim() || !draft.body.trim() || recipientError || pending.current) return;
    pending.current = true;
    setBusy(true);
    setError("");
    setMessage("");
    try {
      // The native command validates confirmed attendees and typed
      // addresses, then hands the draft to the default email client.
      await invoke("open_reviewed_follow_up_draft", {
        meetingId, recipients: selected, manualRecipients: recipients.filter(email => !selected.includes(email)), subject: draft.subject, body: draft.body,
      });
      setMessage("Draft opened. Review and send it from your email client.");
    } catch (failure) {
      setError(typeof failure === "string" ? failure : "Could not open your default email client.");
    } finally {
      pending.current = false;
      setBusy(false);
    }
  }

  async function copyDraft() {
    if (!draft) return;
    setError("");
    setMessage("");
    try {
      await navigator.clipboard.writeText([draft.subject, draft.body].filter(Boolean).join("\n\n"));
      setMessage("Draft copied.");
    } catch {
      setError("Could not copy the draft. You can select and copy the text in the fields.");
    }
  }

  return <Dialog open onOpenChange={open => { if (!open && !pending.current) close(); }}>
    <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-lg">
      <DialogHeader>
        <DialogTitle>Review follow-up email</DialogTitle>
        <DialogDescription>Edit the draft before opening your email client.</DialogDescription>
      </DialogHeader>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      {!draft && (error ? <Button variant="outline" onClick={() => setAttempt(value => value + 1)}>Retry</Button>
        : <p role="status" className="text-sm text-muted-foreground">Loading email draft…</p>)}
      {draft && <form className="space-y-3" onSubmit={event => { event.preventDefault(); void openDraft(); }}>
        {draft.recipients.length ? <fieldset disabled={busy} className="space-y-2">
          <legend className="mb-2 text-sm font-medium">Confirmed attendees</legend>
          {draft.recipients.map((recipient, index) => <div key={recipient.email} className="flex items-start gap-2">
            <Checkbox id={`follow-up-recipient-${index}`} checked={selected.includes(recipient.email)}
              disabled={selected.length >= MAX_RECIPIENTS && !selected.includes(recipient.email)}
              onCheckedChange={checked => setSelected(current => checked === true
                ? [...current, recipient.email] : current.filter(email => email !== recipient.email))} />
            <Label htmlFor={`follow-up-recipient-${index}`} className="min-w-0 break-all">
              {recipient.name ? `${recipient.name} · ${recipient.email}` : recipient.email}
            </Label>
          </div>)}
        </fieldset> : null}
        <div className="space-y-1">
          <Label htmlFor="follow-up-manual-recipients">{draft.recipients.length ? 'Other recipients' : 'Recipients'} (optional)</Label>
          <Input id="follow-up-manual-recipients" disabled={busy} value={manualRecipients}
            autoComplete="off" spellCheck={false} maxLength={MAX_RECIPIENTS * 255}
            aria-invalid={!!recipientError} aria-describedby="follow-up-recipient-hint"
            onChange={event => setManualRecipients(event.target.value)} />
          <p id="follow-up-recipient-hint" className={recipientError ? 'text-sm text-destructive' : 'text-sm text-muted-foreground'}>
            {recipientError || 'Separate email addresses with a comma or semicolon.'}
          </p>
        </div>
        <div className="space-y-1">
          <Label htmlFor="follow-up-subject">Subject</Label>
          <Input id="follow-up-subject" disabled={busy} required maxLength={220} value={draft.subject}
            onChange={event => setDraft({ ...draft, subject: event.target.value })} />
        </div>
        <div className="space-y-1">
          <Label htmlFor="follow-up-body">Message</Label>
          <Textarea id="follow-up-body" disabled={busy} required rows={7} maxLength={8000} value={draft.body}
            onChange={event => setDraft({ ...draft, body: event.target.value })} />
        </div>
        {message && <p role="status" className="text-sm text-muted-foreground">{message}</p>}
        <DialogFooter>
          <Button type="button" variant="outline" disabled={busy} onClick={() => void copyDraft()}>Copy draft</Button>
          <Button type="submit" disabled={busy || !!recipientError || !draft.subject.trim() || !draft.body.trim()}>
            {busy ? "Opening…" : "Open in email client"}
          </Button>
        </DialogFooter>
      </form>}
    </DialogContent>
  </Dialog>;
}
