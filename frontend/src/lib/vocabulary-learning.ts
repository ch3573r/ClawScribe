"use client";

import { invoke } from "@tauri-apps/api/core";

/**
 * Opt-in learning from transcript corrections. When the person turns it on,
 * a saved correction that replaces a misheard word with a name or term offers
 * to add that spelling to the custom vocabulary. Nothing is added without a
 * click, and the offer can be undone.
 */

export const LEARN_VOCABULARY_KEY = "clawscribe.learnVocabularyFromCorrections";
export const MAX_VOCABULARY_WORDS = 500;
const MAX_WORD_LENGTH = 64;
const MAX_SUGGESTIONS = 3;
const MAX_TOKENS = 400;
const MAX_PHRASE_TOKENS = 3;

type FlagStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;

function browserStorage(): FlagStorage | null {
  try {
    return typeof window !== "undefined" ? window.localStorage : null;
  } catch {
    return null;
  }
}

export function readLearnVocabulary(storage: FlagStorage | null = browserStorage()): boolean {
  try {
    return storage?.getItem(LEARN_VOCABULARY_KEY) === "true";
  } catch {
    return false;
  }
}

export function writeLearnVocabulary(enabled: boolean, storage: FlagStorage | null = browserStorage()): boolean {
  if (!storage) return false;
  try {
    if (enabled) storage.setItem(LEARN_VOCABULARY_KEY, "true");
    else storage.removeItem(LEARN_VOCABULARY_KEY);
    return true;
  } catch {
    return false;
  }
}

/** The same rules the vocabulary settings and the core apply to a word. */
export function isValidVocabularyWord(word: string): boolean {
  return word === word.trim() && Array.from(word).length > 0 && Array.from(word).length <= MAX_WORD_LENGTH
    && !/[\u0000-\u001f\u007f-\u009f]/.test(word) && /[\p{Alphabetic}\p{N}]/u.test(word);
}

interface Token { text: string; start: number; end: number }

function tokenize(text: string): Token[] {
  return Array.from(text.matchAll(/[\p{L}\p{N}][\p{L}\p{N}'’-]*/gu), match => ({
    text: match[0].replace(/['’-]+$/, ""), start: match.index ?? 0, end: (match.index ?? 0) + match[0].replace(/['’-]+$/, "").length,
  }));
}

/** Matching token pairs of a longest common subsequence. */
function alignTokens(before: Token[], after: Token[]): [number, number][] {
  const width = after.length + 1;
  const table = new Uint16Array((before.length + 1) * width);
  for (let i = before.length - 1; i >= 0; i--) {
    for (let j = after.length - 1; j >= 0; j--) {
      table[i * width + j] = before[i].text === after[j].text
        ? table[(i + 1) * width + j + 1] + 1
        : Math.max(table[(i + 1) * width + j], table[i * width + j + 1]);
    }
  }
  const pairs: [number, number][] = [];
  for (let i = 0, j = 0; i < before.length && j < after.length;) {
    if (before[i].text === after[j].text) { pairs.push([i, j]); i++; j++; }
    else if (table[(i + 1) * width + j] >= table[i * width + j + 1]) i++;
    else j++;
  }
  return pairs;
}

// A name or term shows an uppercase letter or a digit; plain lowercase words do not.
const looksLikeTerm = (value: string) => /\p{Lu}|\p{N}/u.test(value);

function startsSentence(text: string, position: number): boolean {
  return /(^|[.!?:\n"“„]\s*)$/.test(text.slice(0, position));
}

function alreadyKnown(word: string, known: readonly string[]): boolean {
  const folded = word.toLocaleLowerCase();
  return known.some(entry => entry.toLocaleLowerCase() === folded);
}

/**
 * Names and terms that a correction introduced in place of other words.
 * Pure insertions, pure deletions, long rewrites and words already in the
 * vocabulary are ignored.
 */
export function vocabularySuggestions(before: string, after: string, existing: readonly string[] = []): string[] {
  const oldTokens = tokenize(before);
  const newTokens = tokenize(after);
  if (!oldTokens.length || !newTokens.length || oldTokens.length > MAX_TOKENS || newTokens.length > MAX_TOKENS) return [];
  const pairs = alignTokens(oldTokens, newTokens);
  const suggestions: string[] = [];
  let previousOld = -1;
  let previousNew = -1;
  for (const [oldIndex, newIndex] of [...pairs, [oldTokens.length, newTokens.length] as [number, number]]) {
    const deleted = oldTokens.slice(previousOld + 1, oldIndex);
    const inserted = newTokens.slice(previousNew + 1, newIndex);
    previousOld = oldIndex;
    previousNew = newIndex;
    if (!deleted.length || !inserted.length || inserted.length > MAX_PHRASE_TOKENS) continue;
    const phrase = after.slice(inserted[0].start, inserted[inserted.length - 1].end);
    // A lone capital at the start of a sentence, such as "the" to "The", is not a term.
    const sentenceStartOnly = inserted.length === 1 && startsSentence(after, inserted[0].start) && !/\p{Lu}|\p{N}/u.test(phrase.slice(1));
    if (!looksLikeTerm(phrase) || sentenceStartOnly || !isValidVocabularyWord(phrase)) continue;
    if (alreadyKnown(phrase, existing) || alreadyKnown(phrase, suggestions)) continue;
    suggestions.push(phrase);
    if (suggestions.length === MAX_SUGGESTIONS) break;
  }
  return suggestions;
}

/** The replacement of a find-and-replace, when it looks like a name or term. */
export function replacementSuggestion(replacement: string, existing: readonly string[] = []): string[] {
  const word = replacement.trim().replace(/\s+/g, " ");
  if (!isValidVocabularyWord(word) || !looksLikeTerm(word) || tokenize(word).length > MAX_PHRASE_TOKENS || alreadyKnown(word, existing)) return [];
  return [word];
}

async function readVocabulary(): Promise<string[]> {
  return invoke<string[]>("get_custom_vocabulary");
}

/** Words that would be new to the saved vocabulary. */
export async function newVocabularyWords(candidates: readonly string[]): Promise<string[]> {
  const saved = await readVocabulary();
  const fresh: string[] = [];
  for (const word of candidates) if (!alreadyKnown(word, saved) && !alreadyKnown(word, fresh)) fresh.push(word);
  return fresh;
}

/** Adds words to the saved vocabulary; returns the ones that were new. */
export async function addVocabularyWords(candidates: readonly string[]): Promise<string[]> {
  const saved = await readVocabulary();
  const fresh = (await newVocabularyWords(candidates)).filter(isValidVocabularyWord);
  if (!fresh.length) return [];
  if (saved.length + fresh.length > MAX_VOCABULARY_WORDS) throw new Error("The vocabulary is full. Remove a word in Settings before adding more.");
  await invoke<string[]>("set_custom_vocabulary", { words: [...saved, ...fresh] });
  return fresh;
}

/** Undo for a just-added suggestion; removes only the words that were added. */
export async function removeVocabularyWords(words: readonly string[]): Promise<void> {
  const saved = await readVocabulary();
  const remove = new Set(words.map(word => word.toLocaleLowerCase()));
  await invoke<string[]>("set_custom_vocabulary", { words: saved.filter(word => !remove.has(word.toLocaleLowerCase())) });
}
