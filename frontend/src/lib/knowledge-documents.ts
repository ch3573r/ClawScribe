import type { EvidenceLocator } from '@/types/knowledge';

const referenceErrors: Record<string, string> = {
  unsupported_format: 'Choose a PDF, DOCX, TXT or Markdown file.',
  format_mismatch: 'The file content does not match its extension.',
  input_limit: 'Reference files must be 25 MiB or smaller.',
  text_limit: 'Reference text or citation data exceeds the 2 MiB limit. Choose a shorter document.',
  page_limit: 'PDF references must contain at most 500 pages.',
  zip_limit: 'The DOCX expands beyond 32 MiB. Reduce embedded content and try again.',
  xml_limit: 'A DOCX XML part exceeds 5 MiB. Split the document and try again.',
  encrypted_pdf: 'Encrypted PDFs are unsupported. Choose an unencrypted copy.',
  no_text: 'No readable text was found. Scanned PDFs need a text layer; OCR is unavailable.',
  malformed: 'The reference file is malformed or contains unsupported XML.',
  invalid_utf8: 'TXT and Markdown references must use valid UTF-8 text.',
  worker_limit: 'Document extraction exceeded 10 seconds or its memory limit. Try a smaller file.',
  cancelled: 'Document import was cancelled.',
  busy: 'Finish recording or transcription before importing a reference document.',
  worker_unavailable: 'The local document worker could not start safely. Try again.',
  file_unavailable: 'The reference file could not be read. Check that it is a regular readable file.',
  storage: 'Reference document storage failed. Check free disk space and try again.',
  not_found: 'The meeting or reference attachment is unavailable.',
};

export function referenceError(error: unknown): string {
  const value = typeof error === 'string' ? error : error instanceof Error ? error.message : '';
  // Only typed public-safe errors may be shown; OS/plugin failures can contain paths.
  return referenceErrors[value] ?? Object.values(referenceErrors).find(message => message === value)
    ?? 'The reference operation failed. Retry, or check local storage permissions.';
}

export function documentAnchor(locator: Extract<EvidenceLocator, {kind:'document'}>): string {
  return locator.page == null ? `Paragraph ${locator.paragraph}` : `Page ${locator.page} · paragraph ${locator.paragraph}`;
}
export function knowledgeProviderLabel(provider?: string): string {
  const names: Record<string,string> = {
    'builtin-ai':'Built-in AI',
    codex:'Codex', ollama:'Ollama', openai:'OpenAI', anthropic:'Anthropic', claude:'Claude', openrouter:'OpenRouter',
    'custom-openai':'OpenAI-compatible provider', openclaw:'OpenClaw', groq:'Groq', gemini:'Gemini',
  };
  return provider ? names[provider] ?? 'Configured provider' : 'No provider selected';
}
