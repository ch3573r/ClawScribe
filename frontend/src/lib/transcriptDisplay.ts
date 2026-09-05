/** Keep transcript wording intact. Language-agnostic filler filters can remove
 * real words (for example German "er" and "um") and change the meaning.
 * Concision belongs in meeting notes, not in the source transcript view.
 */
export function formatTranscriptDisplayText(text: string): string {
  return text.trim() || "[Silence]";
}
