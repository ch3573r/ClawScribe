"use client";

export interface ConfluenceDraftInput {
  meetingId: string;
  meetingTitle: string;
  meetingCreatedAt?: string | null;
  summaryMarkdown: string;
}

export type ClipboardWriteMode = "rich" | "markdown";

function formatDateTime(value?: string | null): string {
  if (!value) return "Unknown";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString([], {
    year: "numeric",
    month: "long",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function buildConfluenceDraftMarkdown({
  meetingId,
  meetingTitle,
  meetingCreatedAt,
  summaryMarkdown,
}: ConfluenceDraftInput): string {
  const title = meetingTitle.trim() || "Untitled meeting";
  const body = summaryMarkdown.trim();
  return [
    `# ${title}`,
    "",
    `**Meeting ID:** ${meetingId}`,
    `**Meeting date:** ${formatDateTime(meetingCreatedAt)}`,
    `**Prepared by:** ClawScribe`,
    "",
    "---",
    "",
    body,
    "",
  ].join("\n");
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function inlineMarkdownToHtml(value: string): string {
  let html = escapeHtml(value);
  html = html.replace(/`([^`]+)`/g, "<code>$1</code>");
  html = html.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  html = html.replace(/\*([^*]+)\*/g, "<em>$1</em>");
  html = html.replace(
    /\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g,
    '<a href="$2">$1</a>',
  );
  return html;
}

export function markdownToConfluenceHtml(markdown: string): string {
  const lines = markdown.replace(/\r\n/g, "\n").split("\n");
  const html: string[] = [];
  let listType: "ul" | "ol" | null = null;

  const closeList = () => {
    if (!listType) return;
    html.push(`</${listType}>`);
    listType = null;
  };

  const openList = (nextType: "ul" | "ol") => {
    if (listType === nextType) return;
    closeList();
    listType = nextType;
    html.push(`<${nextType}>`);
  };

  for (const rawLine of lines) {
    const line = rawLine.trimEnd();
    const trimmed = line.trim();

    if (!trimmed) {
      closeList();
      continue;
    }

    const heading = /^(#{1,6})\s+(.+)$/.exec(trimmed);
    if (heading) {
      closeList();
      const level = Math.min(heading[1].length, 6);
      html.push(`<h${level}>${inlineMarkdownToHtml(heading[2])}</h${level}>`);
      continue;
    }

    if (/^[-*_]{3,}$/.test(trimmed)) {
      closeList();
      html.push("<hr />");
      continue;
    }

    const bullet = /^[-*]\s+(.+)$/.exec(trimmed);
    if (bullet) {
      openList("ul");
      html.push(`<li>${inlineMarkdownToHtml(bullet[1])}</li>`);
      continue;
    }

    const numbered = /^\d+\.\s+(.+)$/.exec(trimmed);
    if (numbered) {
      openList("ol");
      html.push(`<li>${inlineMarkdownToHtml(numbered[1])}</li>`);
      continue;
    }

    closeList();
    html.push(`<p>${inlineMarkdownToHtml(trimmed)}</p>`);
  }

  closeList();

  return `<div data-clawscribe-confluence-draft="true">${html.join("\n")}</div>`;
}

export async function writeConfluenceDraftToClipboard(
  markdown: string,
): Promise<ClipboardWriteMode> {
  const html = markdownToConfluenceHtml(markdown);
  const ClipboardItemCtor = (window as any).ClipboardItem;

  if (navigator.clipboard?.write && ClipboardItemCtor) {
    const item = new ClipboardItemCtor({
      "text/html": new Blob([html], { type: "text/html" }),
      "text/plain": new Blob([markdown], { type: "text/plain" }),
    });
    await navigator.clipboard.write([item]);
    return "rich";
  }

  await navigator.clipboard.writeText(markdown);
  return "markdown";
}
