"use client";

import { type FormEvent, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowUpToLine, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Switch } from "@/components/ui/switch";
import { Meta, SettingsCard, SettingsFooter, SettingsRow } from "@/components/ui/settings-layout";
import { readLearnVocabulary, writeLearnVocabulary } from "@/lib/vocabulary-learning";
import { mergeVocabulary, VOCABULARY_LIMIT, VOCABULARY_TERM_LIMIT } from "@/lib/vocabulary-list";

export function CustomVocabularySettings() {
  const [words, setWords] = useState<string[] | null>(null);
  const [draft, setDraft] = useState("");
  const [pasteOpen, setPasteOpen] = useState(false);
  const [pasteDraft, setPasteDraft] = useState("");
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

  async function save(next: string[], action: "add" | "paste" | "remove" | "prioritize") {
    if (inFlight.current) return;
    inFlight.current = true;
    setSaving(true);
    setError(null);
    setStatus("");
    try {
      const saved = await invoke<string[]>("set_custom_vocabulary", { words: next });
      if (mounted.current) {
        setWords(saved);
        if (action === "add") setDraft("");
        if (action === "paste") { setPasteDraft(""); setPasteOpen(false); }
        setStatus(action === "add" ? "Word added." : action === "paste"
          ? `${next.length - (words?.length ?? 0)} terms added.`
          : action === "prioritize" ? "Term moved to the front for Whisper." : "Word removed.");
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

  function add(event: FormEvent<HTMLFormElement>, bulk = false) {
    event.preventDefault();
    if (words === null || inFlight.current) return;
    try {
      void save(mergeVocabulary(words, bulk ? pasteDraft : draft, bulk), bulk ? "paste" : "add");
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : "Could not add vocabulary.");
      setStatus("");
    }
  }

  return (
    <SettingsCard title="Custom vocabulary" description="Corrects similar words to your spelling. Whisper also uses the first terms in the list to guide recognition." aria-busy={loading || saving}>
      <SettingsRow title="Suggest words from my corrections" htmlFor="learn-vocabulary"
        description="After you correct a name or term, offer to add its spelling here. Nothing is added until you confirm.">
        <Switch id="learn-vocabulary" checked={learn} onCheckedChange={enabled => {
          if (writeLearnVocabulary(enabled)) setLearn(enabled);
          else setError("Could not save this choice. Browser storage is unavailable.");
        }} />
      </SettingsRow>
      {loading && <p role="status" className="px-5 py-3 text-sm text-muted-foreground">Loading custom vocabulary…</p>}
      {words !== null && <>
        <form onSubmit={event => add(event)} className="flex flex-wrap items-center gap-2 px-5 py-4">
          <Input ref={input} id="custom-vocabulary-word" value={draft} aria-label="Word or phrase"
            onChange={event => setDraft(event.target.value)} maxLength={VOCABULARY_TERM_LIMIT * 2} className="min-w-40 flex-1"
            placeholder="For example, ChargeBee" autoComplete="off"
            aria-describedby="custom-vocabulary-limit" disabled={loading || saving}
            onKeyDown={event => { if (event.nativeEvent.isComposing && event.key === "Enter") event.preventDefault(); }} />
          <Button type="submit" variant="outline" disabled={loading || saving || !draft.trim()}>Add</Button>
          <Button type="button" variant="ghost" aria-expanded={pasteOpen} aria-controls="custom-vocabulary-paste"
            disabled={loading || saving} onClick={() => setPasteOpen(value => !value)}>{pasteOpen ? "Hide paste list" : "Paste a list"}</Button>
        </form>
        {pasteOpen && <form id="custom-vocabulary-paste" onSubmit={event => add(event, true)} className="space-y-3 px-5 py-4">
          <label className="block space-y-1 text-sm">
            <span>One term per line or separated by commas</span>
            <Textarea value={pasteDraft} onChange={event => setPasteDraft(event.target.value)}
              maxLength={VOCABULARY_LIMIT * (VOCABULARY_TERM_LIMIT * 2 + 2)}
              aria-describedby="custom-vocabulary-limit" disabled={saving} />
          </label>
          <Button type="submit" disabled={saving || !pasteDraft.trim()}>Add terms</Button>
        </form>}
        {words.length > 0 && <ul aria-label="Custom vocabulary words" className="flex flex-wrap gap-2 px-5 py-3">
          {words.map((word, index) => <li key={word} className="inline-flex max-w-full items-center gap-1 rounded-md border border-border py-1 pl-3 pr-1 text-sm">
            <span className="min-w-0 break-all">{word}</span>
            {index > 0 && <Tooltip>
              <TooltipTrigger asChild><Button type="button" variant="ghost" size="icon" className="h-7 w-7 shrink-0"
                aria-label={`Prioritize ${word} for Whisper`} disabled={loading || saving}
                onClick={() => void save([word, ...words.filter(saved => saved !== word)], "prioritize")}>
                <ArrowUpToLine className="h-4 w-4" aria-hidden="true" />
              </Button></TooltipTrigger>
              <TooltipContent>Move to the front for Whisper</TooltipContent>
            </Tooltip>}
            <Button type="button" variant="ghost" size="icon" className="h-7 w-7 shrink-0"
              aria-label={`Remove ${word}`} disabled={loading || saving}
              onClick={() => void save(words.filter(saved => saved !== word), "remove")}>
              <X className="h-4 w-4" aria-hidden="true" />
            </Button>
          </li>)}
        </ul>}
      </>}
      {error && <p role="alert" className="px-5 py-3 text-sm text-destructive">{error}</p>}
      {!loading && words === null && <div className="px-5 py-3"><Button variant="outline" onClick={() => void load()}>Retry</Button></div>}
      <SettingsFooter>
        {words !== null && <Meta><span id="custom-vocabulary-limit">{words.length} of {VOCABULARY_LIMIT} terms · {VOCABULARY_TERM_LIMIT} characters each</span></Meta>}
        {words !== null && <span>{words.length === 0
          ? "No words yet. Add names or terms that are often transcribed wrongly"
          : "Names or terms that are often transcribed wrongly, up to 64 characters each"}</span>}
        <p role="status">{saving ? "Saving vocabulary…" : status}</p>
      </SettingsFooter>
    </SettingsCard>
  );
}
