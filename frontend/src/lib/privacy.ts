import { canSendWebhook, type WebhookConfig } from "./webhook";

/**
 * Derives the Privacy page from the real settings. State goes in, rows come
 * out: nothing here is fixed text that could be wrong for the current setup.
 *
 * Tones: neutral = never leaves this PC, active = only on the user's action,
 * warning = automatic or audio leaving, off = off or not connected.
 */

export type PrivacyTone = "neutral" | "active" | "warning" | "off";

/** A value that is still loading, could not be read, or is known. */
export type Loadable<T> = { status: "loading" } | { status: "error" } | { status: "ready"; value: T };

export type PrivacyState = "ready" | "loading" | "unavailable";

export interface PrivacyInput {
  recording: Loadable<{ saveFolder: string; saveAudio: boolean }>;
  transcription: Loadable<{ provider: string; baseUrl: string | null }>;
  /** The "Cloud transcription" switch on the Transcription page. */
  cloudTranscription: boolean;
  summary: Loadable<{ provider: string | null; ollamaEndpoint: string | null; customEndpoint: string | null }>;
  /** "Automatic summaries" on the Summary page. */
  autoSummary: boolean;
  liveSharing: Loadable<boolean>;
  microsoft: Loadable<{ connected: boolean; calendar: boolean }>;
  /** Confluence is set to publish through its REST API with a base URL. */
  confluenceRest: boolean;
  calendars: Loadable<{ files: number; subscriptions: number }>;
  assistant: Loadable<boolean>;
  /** "Check for updates automatically" on the General page. */
  updateCheck: boolean;
  semanticSearch: Loadable<boolean>;
  webhook: Loadable<WebhookConfig>;
}

export interface PrivacyRow {
  id: string;
  title: string;
  detail: string | null;
  /** The detail is a file path and is shown in the monospace style. */
  detailIsPath: boolean;
  destination: string;
  when: string;
  tone: PrivacyTone;
  state: PrivacyState;
  /** Settings page that controls this row, for `open-settings-tab`. */
  tab: string;
  tabLabel: string;
}

export interface PrivacyFact {
  id: "audio" | "text" | "automatic";
  label: string;
  value: string;
  detail: string | null;
  tone: PrivacyTone;
  state: PrivacyState;
}

export interface PrivacyModel {
  facts: PrivacyFact[];
  rows: PrivacyRow[];
}

const LOCAL_TRANSCRIPTION = new Set(["localWhisper", "parakeet", "nemotron"]);
const ENGINE_NAMES: Record<string, string> = {
  localWhisper: "Whisper",
  parakeet: "Parakeet",
  nemotron: "Nemotron",
  "cloud-whisper": "Hosted Whisper",
  "mai-transcribe": "MAI-Transcribe",
  deepgram: "Deepgram",
  elevenLabs: "ElevenLabs",
  groq: "Groq",
  openai: "OpenAI",
};

export function endpointHost(endpoint: string | null | undefined): string | null {
  if (!endpoint) return null;
  const match = /^[a-z][a-z0-9+.-]*:\/\/(?:[^@/?#]*@)?(\[[^\]]+\]|[^:/?#]+)/i.exec(endpoint.trim());
  return match ? match[1].toLowerCase() : null;
}

export function isLoopbackHost(host: string): boolean {
  return host === "localhost" || host.endsWith(".localhost") || /^127\./.test(host) || host === "::1" || host === "[::1]";
}

export type ProviderKind = "none" | "local" | "remote" | "unknown";
export interface ProviderClass {
  kind: ProviderKind;
  label: string;
}

/**
 * Where a summary provider sends text. "unknown" means the code cannot tell
 * from the provider alone, and the page then says so instead of guessing.
 */
export function classifySummaryProvider(
  provider: string | null | undefined,
  endpoints: { ollamaEndpoint?: string | null; customEndpoint?: string | null } = {},
): ProviderClass {
  if (!provider) return { kind: "none", label: "No provider chosen" };
  switch (provider) {
    case "builtin-ai":
    case "local-llama":
    case "localllama":
      return { kind: "local", label: "Built-in AI" };
    case "ollama": {
      const raw = endpoints.ollamaEndpoint;
      if (!raw || !raw.trim()) return { kind: "local", label: "Ollama on this PC" };
      const host = endpointHost(raw);
      if (!host) return { kind: "unknown", label: "Ollama" };
      return isLoopbackHost(host) ? { kind: "local", label: "Ollama on this PC" } : { kind: "remote", label: `Ollama on ${host}` };
    }
    case "custom-openai": {
      const host = endpointHost(endpoints.customEndpoint);
      if (!host) return { kind: "unknown", label: "OpenAI-compatible provider" };
      return isLoopbackHost(host)
        ? { kind: "local", label: "OpenAI-compatible server on this PC" }
        : { kind: "remote", label: `OpenAI-compatible server on ${host}` };
    }
    case "claude": return { kind: "remote", label: "Anthropic" };
    case "claude-code": return { kind: "remote", label: "Claude Code" };
    case "codex": return { kind: "remote", label: "Codex" };
    case "openai": return { kind: "remote", label: "OpenAI" };
    case "openrouter": return { kind: "remote", label: "OpenRouter" };
    case "groq": return { kind: "remote", label: "Groq" };
    case "gemini": return { kind: "remote", label: "Gemini" };
    default: return { kind: "unknown", label: "Configured provider" };
  }
}

const UNKNOWN_DESTINATION = "Depends on the provider";

/** Whether the rows that depend on these values can be shown yet. */
function blocked(...values: Loadable<unknown>[]): PrivacyState {
  if (values.some((value) => value.status === "error")) return "unavailable";
  if (values.some((value) => value.status === "loading")) return "loading";
  return "ready";
}

const plural = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`;

function row(partial: Pick<PrivacyRow, "id" | "title" | "tab" | "tabLabel"> & Partial<PrivacyRow>): PrivacyRow {
  return { detail: null, detailIsPath: false, destination: "", when: "", tone: "neutral", state: "ready", ...partial };
}

/** A row whose state could not be read yet or at all. */
function withheld(base: PrivacyRow, state: PrivacyState): PrivacyRow {
  return state === "ready"
    ? base
    : { ...base, state, destination: state === "loading" ? "Checking…" : "Could not check", when: "", tone: "off" };
}

interface TranscriptionOutcome {
  leaves: boolean;
  engine: string;
  detail: string;
}

function transcriptionOutcome(input: PrivacyInput): TranscriptionOutcome | null {
  if (input.transcription.status !== "ready") return null;
  const { provider, baseUrl } = input.transcription.value;
  const engine = ENGINE_NAMES[provider] ?? "The selected engine";
  const isLocalEngine = LOCAL_TRANSCRIPTION.has(provider);
  const host = provider === "cloud-whisper" ? endpointHost(baseUrl) : null;
  const onThisPc = isLocalEngine || (host !== null && isLoopbackHost(host));
  if (onThisPc) {
    return {
      leaves: false,
      engine,
      detail: `${engine} · cloud transcription is ${input.cloudTranscription ? "on, but this engine runs on this PC" : "off"}`,
    };
  }
  if (!input.cloudTranscription) {
    return {
      leaves: false,
      engine,
      detail: `${engine} is saved, but cloud transcription is off, so this PC's engine is used`,
    };
  }
  return { leaves: true, engine, detail: `${engine} · cloud transcription is on` };
}

export function derivePrivacy(input: PrivacyInput): PrivacyModel {
  const summaryClass = input.summary.status === "ready"
    ? classifySummaryProvider(input.summary.value.provider, {
        ollamaEndpoint: input.summary.value.ollamaEndpoint,
        customEndpoint: input.summary.value.customEndpoint,
      })
    : null;
  const textLeavesKind = summaryClass && (summaryClass.kind === "remote" || summaryClass.kind === "unknown");
  const outcome = transcriptionOutcome(input);

  // Recordings and audio
  const recordingBase = input.recording.status === "ready"
    ? row({
        id: "recording", title: "Recordings and audio", tab: "recording", tabLabel: "Recording",
        destination: "This PC", when: "Never leaves", tone: "neutral",
        detail: input.recording.value.saveAudio ? input.recording.value.saveFolder || null : "Audio is not saved",
        detailIsPath: input.recording.value.saveAudio && !!input.recording.value.saveFolder,
      })
    : row({ id: "recording", title: "Recordings and audio", tab: "recording", tabLabel: "Recording" });

  // Transcription
  const transcriptionBase = outcome
    ? row({
        id: "transcription", title: "Transcription", tab: "transcription", tabLabel: "Transcription", detail: outcome.detail,
        ...(outcome.leaves
          ? { destination: outcome.engine, when: "When you import or enhance", tone: "warning" as const }
          : { destination: "This PC", when: "Never leaves", tone: "neutral" as const }),
      })
    : row({ id: "transcription", title: "Transcription", tab: "transcription", tabLabel: "Transcription" });

  // Meeting search
  const searchDetail = input.semanticSearch.status === "ready"
    ? input.semanticSearch.value ? "Keyword and semantic index" : "Keyword index"
    : "Local search index";
  const memoryRow = row({
    id: "memory", title: "Meeting search", tab: "memory", tabLabel: "Meeting memory",
    destination: "This PC", when: "Never leaves", tone: "neutral", detail: searchDetail,
  });

  // Summaries
  const summaryDetail = "Transcript text, your template and questions you ask about meetings";
  let summaryBase = row({ id: "summary", title: "Summaries", tab: "summary", tabLabel: "Summary", detail: summaryDetail });
  if (summaryClass) {
    if (summaryClass.kind === "none") {
      summaryBase = { ...summaryBase, destination: "No provider chosen", when: "Nothing is sent", tone: "off" };
    } else if (summaryClass.kind === "local") {
      summaryBase = { ...summaryBase, destination: summaryClass.label, when: "Never leaves", tone: "neutral" };
    } else {
      summaryBase = {
        ...summaryBase,
        destination: summaryClass.kind === "unknown" ? UNKNOWN_DESTINATION : summaryClass.label,
        detail: summaryClass.kind === "unknown" ? `${summaryClass.label} · ${summaryDetail}` : summaryDetail,
        when: input.autoSummary ? "When a recording ends" : "When you generate",
        tone: input.autoSummary ? "warning" : "active",
      };
    }
  }

  // Live assistance
  let liveState: PrivacyState;
  let liveBase = row({ id: "live", title: "Live assistance", tab: "summary", tabLabel: "Summary", detail: "Finalized transcript text" });
  if (input.liveSharing.status !== "ready") {
    liveState = blocked(input.liveSharing);
  } else if (!input.liveSharing.value) {
    liveState = "ready";
    liveBase = { ...liveBase, destination: "Off", when: "Off", tone: "off", detail: "Live transcript sharing is off" };
  } else {
    liveState = blocked(input.summary);
    if (summaryClass) {
      if (summaryClass.kind === "none") liveBase = { ...liveBase, destination: "No provider chosen", when: "Nothing is sent", tone: "off" };
      else if (summaryClass.kind === "local") liveBase = { ...liveBase, destination: summaryClass.label, when: "Never leaves", tone: "neutral" };
      else liveBase = {
        ...liveBase,
        destination: summaryClass.kind === "unknown" ? UNKNOWN_DESTINATION : summaryClass.label,
        detail: summaryClass.kind === "unknown" ? `${summaryClass.label} · Finalized transcript text` : "Finalized transcript text",
        when: "When you ask", tone: "active",
      };
    }
  }

  // Exports
  const exportTargets: string[] = [];
  if (input.microsoft.status === "ready" && input.microsoft.value.connected) exportTargets.push("Microsoft 365");
  if (input.confluenceRest) exportTargets.push("Confluence");
  const confluenceOnly = exportTargets.length === 1 && exportTargets[0] === "Confluence";
  const exportsBase = exportTargets.length
    ? row({
        id: "exports", title: "Exports", tab: confluenceOnly ? "confluence" : "ms365", tabLabel: confluenceOnly ? "Confluence" : "Microsoft 365",
        destination: exportTargets.join(", "), when: "When you export", tone: "active", detail: "Notes, tasks and files you export",
      })
    : row({
        id: "exports", title: "Exports", tab: "ms365", tabLabel: "Microsoft 365",
        destination: "Not connected", when: "Nothing is sent", tone: "off", detail: "Notes, tasks and files you export",
      });
  const exportsState = blocked(input.microsoft);

  const webhook = input.webhook.status === "ready" ? input.webhook.value : null;
  const webhookActive = canSendWebhook(webhook) ? webhook : null;
  const webhookHost = endpointHost(webhookActive?.destination);
  const webhookLeaves = !!webhookHost && !isLoopbackHost(webhookHost);
  const webhookTriggers = [
    webhookActive?.onSummary && "a summary is ready",
    webhookActive?.onRecording && "a recording or import is saved",
  ].filter(Boolean).join(" or ");
  const webhookAuto = webhookLeaves && !!webhookTriggers;
  const webhookText = webhookLeaves && webhookActive?.includeTranscript;
  const webhookContents = [
    "Meeting details",
    webhookActive?.includeSummary && "summary and action items",
    webhookActive?.includeTranscript && "transcript text",
  ].filter(Boolean).join(", ");
  const webhookBase = row({
    id: "webhook", title: "Webhook", tab: "webhook", tabLabel: "Webhook",
    destination: webhookActive?.destination ?? (webhook?.enabled ? "Destination unavailable" : "Off"),
    when: webhookActive ? (webhookTriggers ? `When ${webhookTriggers} or you export` : "When you export") : "Off",
    tone: webhookActive ? (webhookLeaves ? (webhookAuto ? "warning" : "active") : "neutral") : "off",
    detail: webhookActive ? webhookContents : "Sends meeting data to your automation endpoint",
  });

  // Calendars
  const calendarsState: PrivacyState = blocked(input.microsoft, input.calendars);
  let calendarsBase = row({ id: "calendars", title: "Calendars", tab: "calendars", tabLabel: "Calendars" });
  if (input.microsoft.status === "ready" && input.calendars.status === "ready") {
    const parts: string[] = [];
    if (input.microsoft.value.calendar) parts.push("Microsoft 365");
    if (input.calendars.value.subscriptions) parts.push(plural(input.calendars.value.subscriptions, "subscription", "subscriptions"));
    if (input.calendars.value.files) parts.push(plural(input.calendars.value.files, "file", "files"));
    const downloads = input.microsoft.value.calendar || input.calendars.value.subscriptions > 0;
    calendarsBase = !parts.length
      ? { ...calendarsBase, destination: "No calendars", when: "Off", tone: "off", detail: "No calendar is connected" }
      : downloads
        ? { ...calendarsBase, destination: parts.join(", "), when: "Download only", tone: "neutral", detail: "Downloads events, sends nothing about meetings" }
        : { ...calendarsBase, destination: parts.join(", "), when: "Never leaves", tone: "neutral", detail: "Reads calendar files on this PC" };
  }
  // Assistant access
  const assistantBase = input.assistant.status === "ready" && input.assistant.value
    ? row({
        id: "assistant", title: "Assistant access", tab: "assistant", tabLabel: "Assistant access",
        destination: "Local apps on this PC", when: "Whenever an app asks", tone: "warning", detail: "Lets local apps read your meeting library",
      })
    : row({
        id: "assistant", title: "Assistant access", tab: "assistant", tabLabel: "Assistant access",
        destination: "Off", when: "Off", tone: "off", detail: "Lets local apps read your meeting library",
      });

  // Update check
  const updateRow = row({
    id: "update", title: "Update check", tab: "general", tabLabel: "General",
    destination: "GitHub", detail: "Asks for the latest version, no meeting data",
    ...(input.updateCheck ? { when: "At startup and periodically", tone: "warning" as const } : { when: "Only when you check", tone: "active" as const }),
  });

  const rows = [
    withheld(recordingBase, blocked(input.recording)),
    withheld(transcriptionBase, blocked(input.transcription)),
    row({
      id: "conversation", title: "Conversation metrics", tab: "recording", tabLabel: "Recording",
      destination: "This PC", when: "Never leaves", tone: "neutral", detail: "Calculated during live transcription and cleared when the recording ends",
    }),
    memoryRow,
    withheld(summaryBase, blocked(input.summary)),
    withheld(liveBase, liveState),
    withheld(exportsBase, exportsState),
    withheld(webhookBase, blocked(input.webhook)),
    withheld(calendarsBase, calendarsState),
    withheld(assistantBase, blocked(input.assistant)),
    updateRow,
  ];

  // Facts
  const audioState = blocked(input.transcription);
  const audioFact: PrivacyFact = audioState !== "ready"
    ? { id: "audio", label: "Audio", value: audioState === "loading" ? "Checking…" : "Could not check", detail: null, tone: "off", state: audioState }
    : outcome?.leaves
      ? { id: "audio", label: "Audio", value: "Leaves this PC", detail: `Imports and Enhance use ${outcome.engine}`, tone: "warning", state: "ready" }
      : { id: "audio", label: "Audio", value: "Stays on this PC", detail: null, tone: "neutral", state: "ready" };

  const textState = blocked(input.summary, input.webhook);
  let textFact: PrivacyFact;
  if (textState !== "ready" || !summaryClass) {
    textFact = { id: "text", label: "Transcript text", value: textState === "loading" ? "Checking…" : "Could not check", detail: null, tone: "off", state: textState === "ready" ? "unavailable" : textState };
  } else if (summaryClass.kind === "none" || summaryClass.kind === "local") {
    textFact = { id: "text", label: "Transcript text", value: "Stays on this PC", detail: null, tone: "neutral", state: "ready" };
  } else {
    const where = summaryClass.kind === "unknown" ? UNKNOWN_DESTINATION : `To ${summaryClass.label}`;
    textFact = input.autoSummary
      ? { id: "text", label: "Transcript text", value: "Leaves automatically", detail: where, tone: "warning", state: "ready" }
      : { id: "text", label: "Transcript text", value: "Leaves when you ask", detail: where, tone: "active", state: "ready" };
  }
  if (webhookText) {
    const automatic = webhookAuto || textFact.tone === "warning";
    textFact = {
      ...textFact,
      value: automatic ? "Leaves automatically" : "Leaves when you ask",
      detail: [textFact.detail, `Webhook on ${webhookHost}`].filter(Boolean).join(" · "),
      tone: automatic ? "warning" : "active",
      state: "ready",
    };
  }

  const automaticState = input.autoSummary ? blocked(input.summary, input.webhook) : blocked(input.webhook);
  let automaticFact: PrivacyFact;
  if (automaticState !== "ready") {
    automaticFact = { id: "automatic", label: "Meeting data sent automatically", value: automaticState === "loading" ? "Checking…" : "Could not check", detail: null, tone: "off", state: automaticState };
  } else if (input.autoSummary && textLeavesKind) {
    automaticFact = { id: "automatic", label: "Meeting data sent automatically", value: "Transcript text", detail: summaryClass && summaryClass.kind === "unknown" ? UNKNOWN_DESTINATION : `To ${summaryClass?.label ?? ""}`, tone: "warning", state: "ready" };
  } else {
    automaticFact = { id: "automatic", label: "Meeting data sent automatically", value: "None", detail: null, tone: "neutral", state: "ready" };
  }
  if (webhookAuto) {
    automaticFact = {
      ...automaticFact,
      value: automaticFact.tone === "warning" || webhookText ? "Meeting details and transcript text" : "Meeting details",
      detail: [automaticFact.detail, `${webhookContents} to ${webhookHost}`].filter(Boolean).join(" · "),
      tone: "warning",
      state: "ready",
    };
  }

  return { facts: [audioFact, textFact, automaticFact], rows };
}
