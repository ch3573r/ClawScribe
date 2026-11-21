"use client";

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { ResolvedSummarySource } from "./SummarySources";
import type { MeetingExportHistory } from "@/services/microsoftExportService";

interface ReviewedAction {
  id: string;
  title: string;
  owner: string | null;
  dueDate: string | null;
  done: boolean;
  sourceKey: string | null;
  legacyAction: { id: string; titleHash: string } | null;
}

type DismissedSuggestion = { id: string; legacyAction: NonNullable<ReviewedAction["legacyAction"]> };

function message(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

export function ReviewedActions({ open, onOpenChange, meetingId, getMarkdown, onReturnFocus, targetActionId }: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  meetingId: string;
  getMarkdown: () => Promise<string>;
  onReturnFocus?: () => void;
  targetActionId?: string | null;
}) {
  const [actions, setActions] = useState<ReviewedAction[]>([]);
  const [dismissed, setDismissed] = useState<DismissedSuggestion[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [revision, setRevision] = useState("");
  const [history, setHistory] = useState<MeetingExportHistory[]>([]);
  const [historyError, setHistoryError] = useState(false);
  const [reload, setReload] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [source, setSource] = useState<{ actionId: string; passage: ResolvedSummarySource } | null>(null);
  const pending = useRef(false);

  useEffect(() => {
    if (!open) return;
    let active = true;
    setActions([]);
    setDismissed([]);
    setLoaded(false);
    setError("");
    setNotice("");
    setSource(null);
    setHistory([]);
    setHistoryError(false);
    void invoke<MeetingExportHistory[]>("get_meeting_export_history", { meetingId }).then(items => {
      if (active) setHistory(items);
    }).catch(() => { if (active) setHistoryError(true); });
    void invoke<{ actions: ReviewedAction[]; revision: string }>("get_reviewed_action_editor", { meetingId }).then(result => {
      if (!active) return;
      setActions(result.actions);
      setRevision(result.revision);
      setLoaded(true);
    }).catch(error => { if (active) setError(message(error)); });
    return () => { active = false; };
  }, [open, meetingId, reload]);

  useEffect(() => {
    if (!open || !loaded || !targetActionId) return;
    document.getElementById(`action-done-${targetActionId}`)?.focus();
  }, [open, loaded, targetActionId]);

  function update(id: string, patch: Partial<ReviewedAction>) {
    setActions(previous => previous.map(action => action.id === id ? { ...action, ...patch } : action));
  }

  async function run(work: () => Promise<void>) {
    if (pending.current) return;
    pending.current = true;
    setBusy(true);
    setError("");
    setNotice("");
    try { await work(); }
    catch (error) { setError(message(error)); }
    finally {
      pending.current = false;
      setBusy(false);
    }
  }

  function importSuggestions() {
    void run(async () => {
      const suggestions = await invoke<ReviewedAction[]>("suggest_meeting_actions", { meetingId, markdown: await getMarkdown(), reviewed: actions, dismissed });
      const additional = suggestions.slice(0, 250 - actions.length);
      setActions([...actions, ...additional]);
      if (!additional.length) setNotice("No new action suggestions found in the meeting summary.");
      else setNotice(`${additional.length} suggestions added. Review them and save your actions.`);
    });
  }

  function addAction() {
    setActions(previous => [...previous, { id: globalThis.crypto.randomUUID(), title: "", owner: null, dueDate: null, done: false, sourceKey: null, legacyAction: null }]);
  }

  function removeAction(action: ReviewedAction) {
    const { id, legacyAction } = action;
    if (legacyAction) setDismissed(previous => [...previous, { id, legacyAction }]);
    setActions(rows => rows.filter(row => row.id !== id));
  }

  function inspectSource(action: ReviewedAction) {
    void run(async () => {
      setSource(null);
      const passage = await invoke<ResolvedSummarySource>("resolve_reviewed_action_source", { meetingId, actionId: action.id, key: action.sourceKey });
      setSource({ actionId: action.id, passage });
    });
  }

  return <Dialog open={open} onOpenChange={value => { if (!pending.current) onOpenChange(value); }}>
    <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-2xl" onCloseAutoFocus={event => {
      if (onReturnFocus) {
        event.preventDefault();
        onReturnFocus();
      }
    }}>
      <DialogHeader>
        <DialogTitle>Meeting actions</DialogTitle>
        <DialogDescription>Save reviewed follow-ups with this meeting. You can edit them offline and review open actions for Planner or Microsoft To Do.</DialogDescription>
      </DialogHeader>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      {!loaded && (error
        ? <Button type="button" variant="outline" onClick={() => setReload(value => value + 1)}>Retry loading</Button>
        : <p role="status" className="text-sm text-muted-foreground">Loading meeting actions…</p>)}
      {loaded && <form className="space-y-4" onSubmit={event => {
        event.preventDefault();
        void run(async () => {
          await invoke("save_reviewed_meeting_actions", { meetingId, actions, dismissed, expectedRevision: revision });
          onOpenChange(false);
        });
      }}>
        <div className="flex flex-wrap gap-2">
          <Button type="button" variant="outline" disabled={busy || actions.length >= 250} onClick={addAction}>Add action</Button>
          <Button type="button" variant="outline" disabled={busy || actions.length >= 250} onClick={importSuggestions}>Add summary suggestions</Button>
        </div>
        {notice && <p role="status" className="text-sm text-muted-foreground">{notice}</p>}
        {historyError && <p role="status" className="text-sm text-muted-foreground">Export history is unavailable. You can still edit your actions.</p>}
        {targetActionId && !actions.some(action => action.id === targetActionId) && <p role="status" className="text-sm text-muted-foreground">The selected action is no longer saved.</p>}
        {!actions.length && <p className="text-sm text-muted-foreground">No actions saved for this meeting.</p>}
        {actions.map(action => <fieldset key={action.id} disabled={busy} className="space-y-2 rounded-md border border-border p-3">
          <Label htmlFor={`action-title-${action.id}`}>Task</Label>
          {[...new Set(history.filter(item => item.actionId === action.id && item.status === "succeeded").map(item => item.destination))]
            .map(destination => <p key={destination} className="text-xs text-muted-foreground">Exported to {destination}</p>)}
          <Input id={`action-title-${action.id}`} required maxLength={255} value={action.title} onChange={event => update(action.id, { title: event.target.value })} />
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1">
              <Label htmlFor={`action-owner-${action.id}`}>Owner</Label>
              <Input id={`action-owner-${action.id}`} maxLength={160} value={action.owner ?? ""} onChange={event => update(action.id, { owner: event.target.value })} />
            </div>
            <div className="space-y-1">
              <Label htmlFor={`action-date-${action.id}`}>Due date</Label>
              <Input id={`action-date-${action.id}`} type="date" value={action.dueDate ?? ""} onChange={event => update(action.id, { dueDate: event.target.value || null })} />
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <Checkbox id={`action-done-${action.id}`} checked={action.done} onCheckedChange={checked => update(action.id, { done: checked === true })} />
            <Label htmlFor={`action-done-${action.id}`}>Done</Label>
            {action.sourceKey && <Button type="button" size="sm" variant="ghost" onClick={() => inspectSource(action)}>View source</Button>}
            <Button type="button" size="sm" variant="ghost" className="ml-auto" aria-label={action.title ? `Remove ${action.title}` : "Remove blank action"} onClick={() => removeAction(action)}>Remove</Button>
          </div>
          {source?.actionId === action.id && <div className="space-y-2">
            {source.passage.stale && <p role="status" className="text-sm text-muted-foreground">This passage has changed since the action was suggested.</p>}
            <blockquote className="whitespace-pre-wrap break-words border-l-2 border-primary pl-3 text-sm">{source.passage.text}</blockquote>
          </div>}
        </fieldset>)}
        <DialogFooter>
          <Button type="button" variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button type="submit" disabled={busy || actions.some(action => !action.title.trim())}>Save actions</Button>
        </DialogFooter>
      </form>}
    </DialogContent>
  </Dialog>;
}
