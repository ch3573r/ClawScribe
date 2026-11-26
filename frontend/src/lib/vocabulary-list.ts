export const VOCABULARY_LIMIT = 500;
export const VOCABULARY_TERM_LIMIT = 64;

export function mergeVocabulary(existing: readonly string[], draft: string, bulk = false): string[] {
  const terms = bulk
    ? draft.split(/\r\n|[\r\n]/).flatMap((line, index) => line.split(",").map((input, position) => ({
        input, location: `Line ${index + 1}, term ${position + 1}. `,
      })))
    : [{ input: draft, location: "" }];
  const seen = new Set(existing.map(term => term.toLowerCase()));
  const merged = [...existing];
  for (const { input, location } of terms) {
    const term = input.trim();
    if (!term) continue;
    if (Array.from(term).length > VOCABULARY_TERM_LIMIT) throw new Error(location + "Use at most 64 characters per word or phrase.");
    if (/[\u0000-\u001f\u007f-\u009f]/.test(term)) throw new Error(location + "Remove control characters or line breaks.");
    if (!/[\p{Alphabetic}\p{N}]/u.test(term)) throw new Error(location + "Use at least one letter or number.");
    const key = term.toLowerCase();
    if (seen.has(key)) continue;
    if (merged.length >= VOCABULARY_LIMIT) throw new Error(location + "Remove a word before adding another. The limit is 500 entries.");
    seen.add(key);
    merged.push(term);
  }
  if (merged.length === existing.length) {
    throw new Error(bulk ? "The list contains no new terms." : "That word is already in your vocabulary.");
  }
  return merged;
}
