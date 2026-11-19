/** What a summary provider receives. Drives the "What this sends" panel from the selected provider. */

export interface ProviderDataItem {
  title: string;
  detail: string;
  /** "active" data leaves this PC; "neutral" never does. */
  tone: "active" | "neutral";
}

export interface ProviderData {
  label: string;
  /** True when nothing is sent off this PC. */
  local: boolean;
  items: ProviderDataItem[];
}

const LABELS: Record<string, string> = {
  "builtin-ai": "Built-in AI",
  ollama: "Ollama",
  "custom-openai": "OpenAI or compatible",
  claude: "Claude",
  groq: "Groq",
  openrouter: "OpenRouter",
  openclaw: "OpenClaw",
  codex: "Codex",
  "claude-code": "Claude Code",
};

export const providerLabel = (provider: string) => LABELS[provider] ?? "the provider";

/** True for an empty endpoint (Ollama's default) or one that points at this PC. */
export function isLocalEndpoint(endpoint?: string | null): boolean {
  const text = endpoint?.trim();
  if (!text) return true;
  const match = /^[a-z][a-z0-9+.-]*:\/\/(?:[^@/?#]*@)?(\[[^\]]+\]|[^:/?#]+)/i.exec(text);
  if (!match) return false;
  const host = match[1].replace(/^\[|\]$/g, "").toLowerCase();
  return host === "localhost" || host === "127.0.0.1" || host === "::1";
}

const AUDIO: ProviderDataItem = { title: "Audio", detail: "Never leaves this PC", tone: "neutral" };

export function describeProviderData(provider: string, ollamaEndpoint?: string | null): ProviderData {
  const label = providerLabel(provider);
  const local = provider === "builtin-ai" || (provider === "ollama" && isLocalEndpoint(ollamaEndpoint));
  if (local) {
    return {
      label,
      local: true,
      items: [{ title: "Nothing leaves this PC", detail: `Summaries are written on this PC with ${label}`, tone: "neutral" }, AUDIO],
    };
  }
  const items: ProviderDataItem[] = [
    { title: "Transcript text", detail: "When you generate a summary or ask a question", tone: "active" },
    { title: "Template and language", detail: "Sent with each summary request", tone: "active" },
  ];
  if (provider === "openclaw") {
    items.push({ title: "Completed meetings", detail: "Submitted to your gateway while handoff is on", tone: "active" });
  }
  items.push(AUDIO);
  return { label, local: false, items };
}
