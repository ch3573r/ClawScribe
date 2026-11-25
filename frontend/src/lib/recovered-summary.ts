import type { EditorSummary } from '@/types';
import { markdownToPlainText } from './clipboard-format';

export function recoveredSummary(notes: string): EditorSummary {
  // A failed run has no verified source map. Save its readable text without
  // introducing links that could be mistaken for the previous run's citations.
  const text = markdownToPlainText(notes).replace(/#clawscribe-source-[0-9a-f]+/gi, 'Source reference');
  const markdown = text.split(/\n+/).map(line => line.trim().replace(/[!-/:-@\[-`{-~]/g, '\\$&')).filter(Boolean).join('\n\n');
  if (!markdown) throw new Error('No readable excerpt notes to save.');
  return { markdown };
}
