import type { EvidenceLocator } from '@/types/knowledge';

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
