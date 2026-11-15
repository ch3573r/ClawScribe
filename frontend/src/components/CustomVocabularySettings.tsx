"use client";

import { type FormEvent, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { PageSection } from "@/components/ui/page-section";

export function CustomVocabularySettings() {
  const [words, setWords] = useState<string[] | null>(null);
  const [draft, setDraft] = useState("");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState("");
  const inFlight = useRef(false);
  const mounted = useRef(false);
  const input = useRef<HTMLInputElement>(null);

  async function load() {
    if (inFlight.current) return;
    inFlight.current = true;
    setLoading(true);
    setError(null);
    try {
      const saved = await invoke<string[]>("get_custom_vocabulary");
      if (mounted.current) setWords(saved);
    } catch (problem) {
      if (mounted.current) setError(typeof problem === "string" ? problem : "Could not load custom vocabulary. Retry to continue.");
    } finally {
      inFlight.current = false;
      if (mounted.current) setLoading(false);
    }
  }

  useEffect(() => {
    mounted.current = true;
    void load();
    return () => { mounted.current = false; };
  }, []);

  useEffect(() => {
    if (!saving && status) input.current?.focus();
  }, [saving, status]);

  async function save(next: string[], adding: boolean) {
    if (inFlight.current) return;
    inFlight.current = true;
    setSaving(true);
    setError(null);
    setStatus("");
    try {
      const saved = await invoke<string[]>("set_custom_vocabulary", { words: next });
      if (mounted.current) {
        setWords(saved);
        if (adding) setDraft("");
        setStatus(adding ? "Word added." : "Word removed.");
      }
    } catch (problem) {
      if (mounted.current) setError(typeof problem === "string" ? problem : "Could not save custom vocabulary. Your list is unchanged; retry the action.");
    } finally {
      inFlight.current = false;
      if (mounted.current) {
        setSaving(false);
      }
    }
  }

  function add(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (words === null || inFlight.current) return;
    const word = draft.trim();
    if (!word) return;
    let problem: string | null = null;
    if (Array.from(word).length > 64) problem = "Use at most 64 characters per word or phrase.";
    else if (/[\u0000-\u001f\u007f-\u009f]/.test(word)) problem = "Remove control characters or line breaks.";
    else if (!/[\p{Alphabetic}\p{N}]/u.test(word)) problem = "Use at least one letter or number.";
    else if (words.some(saved => saved.toLowerCase() === word.toLowerCase())) problem = "That word is already in your vocabulary.";
    else if (words.length >= 500) problem = "Remove a word before adding another; the limit is 500 entries.";
    if (problem) {
      setError(problem);
      setStatus("");
      return;
    }
    void save([...words, word], true);
  }

  return (
    <PageSection title="Custom vocabulary" description="Corrects similar words to your spelling in new transcripts and retranscription.">
      <div className="space-y-4 p-5" aria-busy={loading || saving}>
        {loading && <p role="status" className="text-sm text-muted-foreground">Loading custom vocabulary…</p>}
        {words !== null && <>
          <form onSubmit={add} className="space-y-2">
            <Label htmlFor="custom-vocabulary-word">Word or phrase</Label>
            <div className="flex items-center gap-2">
              <Input ref={input} id="custom-vocabulary-word" value={draft}
                onChange={event => setDraft(event.target.value)} maxLength={128}
                placeholder="For example, ChargeBee" autoComplete="off"
                aria-describedby="custom-vocabulary-limit" disabled={loading || saving}
                onKeyDown={event => { if (event.nativeEvent.isComposing && event.key === "Enter") event.preventDefault(); }} />
              <Button type="submit" disabled={loading || saving || !draft.trim()}>Add</Button>
            </div>
            <p id="custom-vocabulary-limit" className="text-xs text-muted-foreground">{words.length} of 500 entries · Up to 64 characters each</p>
          </form>
          {words.length === 0
            ? <p className="text-sm text-muted-foreground">No words yet. Add names or terms that are often transcribed incorrectly.</p>
            : <ul aria-label="Custom vocabulary words" className="flex flex-wrap gap-2">
              {words.map(word => <li key={word} className="inline-flex max-w-full items-center gap-1 rounded-md border border-border py-1 pl-3 pr-1 text-sm">
                <span className="min-w-0 break-all">{word}</span>
                <Button type="button" variant="ghost" size="icon" className="h-7 w-7 shrink-0"
                  aria-label={`Remove ${word}`} disabled={loading || saving}
                  onClick={() => void save(words.filter(saved => saved !== word), false)}>
                  <X className="h-4 w-4" aria-hidden="true" />
                </Button>
              </li>)}
            </ul>}
        </>}
        {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
        {!loading && words === null && <Button variant="outline" onClick={() => void load()}>Retry</Button>}
        <p role="status" className="text-sm text-muted-foreground">{saving ? "Saving vocabulary…" : status}</p>
      </div>
    </PageSection>
  );
}
