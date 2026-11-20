"use client";

import { toast } from "sonner";
import { addVocabularyWords, newVocabularyWords, readLearnVocabulary, removeVocabularyWords } from "@/lib/vocabulary-learning";

const quote = (words: readonly string[]) => words.map(word => `“${word}”`).join(", ");

async function add(words: string[]) {
  try {
    const added = await addVocabularyWords(words);
    if (!added.length) return;
    toast.success(`Added ${quote(added)} to your vocabulary`, {
      id: "vocabulary-added",
      action: { label: "Undo", onClick: () => { void removeVocabularyWords(added).catch(() => toast.error("Could not undo. Remove the word in Settings.")); } },
    });
  } catch (error) {
    toast.error("Could not add to your vocabulary", { description: error instanceof Error ? error.message : undefined });
  }
}

/**
 * After a correction, offer to add the words it introduced to the custom
 * vocabulary. Does nothing unless the person turned this on, and adds nothing
 * until they confirm.
 */
export async function offerVocabularyWords(candidates: readonly string[]): Promise<void> {
  if (!candidates.length || !readLearnVocabulary()) return;
  let fresh: string[];
  try { fresh = await newVocabularyWords(candidates); }
  catch { return; } // The correction itself is saved; the offer is optional.
  if (!fresh.length) return;
  toast(`Add ${quote(fresh)} to your vocabulary?`, {
    id: "vocabulary-suggestion",
    description: "Future transcripts will use this spelling.",
    duration: 12000,
    action: { label: "Add", onClick: () => { void add(fresh); } },
  });
}
