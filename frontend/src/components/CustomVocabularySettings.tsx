"use client";

import { type FormEvent, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Meta, SettingsCard, SettingsFooter, SettingsRow } from "@/components/ui/settings-layout";
import { readLearnVocabulary, writeLearnVocabulary } from "@/lib/vocabulary-learning";

export function CustomVocabularySettings() {
  const [words, setWords] = useState<string[] | null>(null);
  const [draft, setDraft] = useState("");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState("");
  const [learn, setLearn] = useState(false);
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
    setLearn(readLearnVocabulary());
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
    <SettingsCard title="Custom vocabulary" description="Corrects similar words to your spelling" aria-busy={loading || saving}>
      <SettingsRow title="Suggest words from my corrections" htmlFor="learn-vocabulary"
        description="After you correct a name or term, offer to add its spelling here. Nothing is added until you confirm.">
        <Switch id="learn-vocabulary" checked={learn} onCheckedChange={enabled => {
          if (writeLearnVocabulary(enabled)) setLearn(enabled);
          else setError("Could not save this choice. Browser storage is unavailable.");
        }} />
      </SettingsRow>
      {loading && <p role="status" className="px-5 py-3 text-sm text-muted-foreground">Loading custom vocabulary…</p>}
      {words !== null && <>
        <form onSubmit={add} className="flex items-center gap-2 px-5 py-4">
          <Input ref={input} id="custom-vocabulary-word" value={draft} aria-label="Word or phrase"
            onChange={event => setDraft(event.target.value)} maxLength={128}
            placeholder="For example, ChargeBee" autoComplete="off"
            aria-describedby="custom-vocabulary-limit" disabled={loading || saving}
            onKeyDown={event => { if (event.nativeEvent.isComposing && event.key === "Enter") event.preventDefault(); }} />
          <Button type="submit" variant="outline" disabled={loading || saving || !draft.trim()}>Add</Button>
        </form>
        {words.length > 0 && <ul aria-label="Custom vocabulary words" className="flex flex-wrap gap-2 px-5 py-3">
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
      {error && <p role="alert" className="px-5 py-3 text-sm text-destructive">{error}</p>}
      {!loading && words === null && <div className="px-5 py-3"><Button variant="outline" onClick={() => void load()}>Retry</Button></div>}
      <SettingsFooter>
        {words !== null && <Meta><span id="custom-vocabulary-limit">{words.length} of 500</span></Meta>}
        {words !== null && <span>{words.length === 0
          ? "No words yet. Add names or terms that are often transcribed wrongly"
          : "Names or terms that are often transcribed wrongly, up to 64 characters each"}</span>}
        <p role="status">{saving ? "Saving vocabulary…" : status}</p>
      </SettingsFooter>
    </SettingsCard>
  );
}
