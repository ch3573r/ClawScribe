"use client";

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { citationParts } from "@/lib/knowledge-state";
import { knowledgeProviderLabel } from "@/lib/knowledge-documents";
import { bookmarkTime } from "@/lib/library";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

interface AnswerNote {
  id: string;
  content: string;
  provider: string;
  model: string;
  context: { session_id: string; finalized_through_seconds: number; transcription_incomplete: boolean };
  cited_tags: number[];
  question_tags: number[];
}

interface Excerpt {
  text: string;
  start_seconds: number | null;
  historical: boolean;
}

export function SavedLiveAnswers({ meetingId }: { meetingId: string }) {
  const [notes, setNotes] = useState<AnswerNote[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [open, setOpen] = useState(false);
  const [error, setError] = useState("");
  const [removing, setRemoving] = useState<AnswerNote | null>(null);
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const [source, setSource] = useState<{ tag: number; text: string; excerpt?: Excerpt | null } | null>(null);
  const sourceRequest = useRef(0);

  useEffect(() => {
    let active = true;
    setError("");
    void invoke<AnswerNote[]>("list_saved_live_answers", { meetingId }).then(data => {
      if (active) { setNotes(data); setLoaded(true); }
    }).catch(() => { if (active) setError("Could not load saved live answers."); });
    return () => { active = false; sourceRequest.current++; };
  }, [meetingId, attempt]);

  async function inspect(noteId: string, tag: number) {
    const current = ++sourceRequest.current;
    setSource({ tag, text: "Checking source…" });
    try {
      const excerpt = await invoke<Excerpt | null>("resolve_saved_live_answer_citation", { meetingId, answerId: noteId, tag });
      if (current === sourceRequest.current) setSource({ tag, text: excerpt ? "" : "This reference is no longer available.", excerpt });
    } catch {
      if (current === sourceRequest.current) setSource({ tag, text: "Could not verify this source. Choose it again to retry." });
    }
  }

  async function remove() {
    if (!removing || pending.current) return;
    pending.current = true;
    setBusy(true);
    setError("");
    try {
      await invoke("delete_saved_live_answer", { meetingId, answerId: removing.id });
      setNotes(previous => previous.filter(note => note.id !== removing.id));
      setRemoving(null);
    } catch {
      setError("Could not remove the saved answer. Try again.");
    } finally {
      pending.current = false;
      setBusy(false);
    }
  }

  if (!notes.length && !open && !error) return null;
  return <div className="border-b border-border px-4 py-2">
    {!loaded && error ? <div className="flex items-center gap-2">
      <p role="alert" className="text-sm text-destructive">{error}</p>
      <Button variant="outline" size="sm" onClick={() => setAttempt(value => value + 1)}>Retry</Button>
    </div> : <Button variant="outline" size="sm" onClick={() => setOpen(true)}>Saved live answers ({notes.length})</Button>}
    <Dialog open={open} onOpenChange={value => {
      if (!pending.current) { setOpen(value); setRemoving(null); setSource(null); setError(""); sourceRequest.current++; }
    }}>
      <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{removing ? "Remove saved answer?" : "Saved live answers"}</DialogTitle>
          <DialogDescription>{removing ? "This removes the selected note and its saved evidence from this meeting." : "Answers you saved during recording and the evidence available when they were generated."}</DialogDescription>
        </DialogHeader>
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        {removing ? <DialogFooter>
          <Button variant="outline" autoFocus disabled={busy} onClick={() => { setRemoving(null); setError(""); }}>Cancel</Button>
          <Button variant="destructive" disabled={busy} onClick={() => void remove()}>{busy ? "Removing…" : "Remove answer"}</Button>
        </DialogFooter> : <>
          {!notes.length && <p className="text-sm text-muted-foreground">No saved live answers.</p>}
          {notes.map(note => <article key={note.id} className="space-y-2 rounded-md border border-border p-3">
            <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
              <Tooltip><TooltipTrigger asChild><Button type="button" size="sm" variant="ghost">{knowledgeProviderLabel(note.provider)}</Button></TooltipTrigger>
                <TooltipContent>{note.model}</TooltipContent></Tooltip>
              <span>Finalized through {bookmarkTime(note.context.finalized_through_seconds)}{note.context.transcription_incomplete ? " · Transcript incomplete" : ""}</span>
            </div>
            <p className="whitespace-pre-wrap break-words text-sm">
              {citationParts(note.content, Math.max(0, ...note.cited_tags), note.cited_tags).map((part, index) => part.tags
                ? part.tags.map(tag => <Button type="button" key={`${index}-${tag}`} variant="ghost" size="sm"
                    className="mx-0.5 inline-flex h-5 min-w-5 align-super px-1 text-xs text-primary"
                    aria-label={`View source ${tag}`} onClick={() => void inspect(note.id, tag)}>{tag}</Button>)
                : part.text.replace(/\[[^\]]*\bK\d+[^\]]*\]/g, "(source unavailable)"))}
            </p>
            <div className="flex flex-wrap items-center gap-2">
              {note.question_tags.map(tag => <Button key={tag} variant="ghost" size="sm" onClick={() => void inspect(note.id, tag)}>View preceding question</Button>)}
              <Button variant="ghost" size="sm" className="ml-auto" onClick={() => {
                sourceRequest.current++; setSource(null); setRemoving(note);
              }}>Remove</Button>
            </div>
          </article>)}
          {source && <section className="sticky bottom-0 space-y-2 rounded-md border border-border bg-card p-3" aria-label="Source passage">
            <div className="flex items-center justify-between gap-2">
              <h3 className="text-sm font-medium">Source {source.tag}</h3>
              <Button variant="ghost" size="sm" onClick={() => { sourceRequest.current++; setSource(null); }}>Close source</Button>
            </div>
            {source.excerpt ? <>
              {source.excerpt.historical && <p className="text-xs text-muted-foreground">Original finalized speech{source.excerpt.start_seconds != null ? ` · ${bookmarkTime(source.excerpt.start_seconds)}` : ""}</p>}
              <blockquote className="whitespace-pre-wrap break-words border-l-2 border-primary pl-3 text-sm">{source.excerpt.text}</blockquote>
            </> : <p role="status" className="text-sm text-muted-foreground">{source.text}</p>}
          </section>}
        </>}
      </DialogContent>
    </Dialog>
  </div>;
}
