"use client";

import type { MeetingExportHeader } from "@/lib/meetingExportContent";

export type ClipboardWriteMode = "rich" | "markdown";

export function buildConfluenceDraft(header: MeetingExportHeader, summaryMarkdown: string): MeetingExportHeader {
  const body = summaryMarkdown.replace(/\[([^\]\n]*)\]\(#clawscribe-source-[0-9a-f]+\)/gi, "($1)").trim();
  return {
    markdown: header.markdown.trimEnd() + "\n\n" + body + "\n",
    html: markdownToConfluenceHtml(body, header.html),
  };
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function inlineMarkdownToHtml(value: string): string {
  const tokens: string[] = [];
  const token = (html: string) => `\u0000${tokens.push(html) - 1}\u0000`;
  const emphasize = (text: string) => text
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
    .replace(/\*([^*]+)\*/g, "<em>$1</em>");
  const formatLabel = (label: string) => label.split(/(`[^`]+`)/g)
    .map((part, index) => index % 2 ? `<code>${part.slice(1, -1)}</code>` : emphasize(part)).join("");
  const text = escapeHtml(value.replace(/\u0000/g, '\uFFFD')).replace(
    /`([^`]+)`|\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g,
    (_match, code: string | undefined, label: string, href: string) => code === undefined
      ? token(`<a href="${href}">${formatLabel(label)}</a>`)
      : token(`<code>${code}</code>`),
  );
  return emphasize(text).replace(/\u0000(\d+)\u0000/g, (_match, index) => tokens[Number(index)]);
}

function splitTableRow(line: string): string[] {
  const cells: string[] = [];
  let cell = "";
  let backslashes = 0;
  let trailingPipe = false;
  for (const char of line.trim().replace(/^\|/, "")) {
    trailingPipe = char === "|" && backslashes % 2 === 0;
    if (trailingPipe) {
      cells.push(cell.trim());
      cell = "";
    } else {
      if (char === "|") cell = cell.slice(0, -1);
      cell += char;
    }
    backslashes = char === "\\" ? backslashes + 1 : 0;
  }
  if (!trailingPipe || !cells.length) cells.push(cell.trim());
  return cells;
}

function isTableDelimiter(line: string | undefined, columns: number): boolean {
  if (line === undefined) return false;
  const cells = splitTableRow(line);
  return cells.length === columns && cells.every(cell => /^:?-{3,}:?$/.test(cell));
}

const LIST_ITEM = /^(\s*)([-*+]|\d+[.)])\s+(?:\[( |x|X)\]\s+)?(.+)$/;

function renderBlocks(lines: string[], quoteDepth = 0): string {
  const html: string[] = [];
  const lists: { indent: number; tag: "ul" | "ol" }[] = [];
  const closeItem = () => { html[html.length - 1] += "</li>"; };
  const closeLists = (indent = -1) => {
    while (lists.length && lists[lists.length - 1].indent > indent) {
      closeItem();
      html.push(`</${lists.pop()!.tag}>`);
    }
  };
  const addItem = (indent: number, tag: "ul" | "ol", content: string, start?: number) => {
    closeLists(indent);
    const current = lists[lists.length - 1];
    if (current?.indent === indent && current.tag !== tag) {
      closeLists(indent - 1);
    }
    const sameList = lists[lists.length - 1]?.indent === indent;
    if (sameList) closeItem();
    else {
      lists.push({ indent, tag });
      html.push(`<${tag}${start !== undefined && start !== 1 ? ` start="${start}"` : ""}>`);
    }
    html.push(`<li>${content}`);
  };

  for (let index = 0; index < lines.length; index++) {
    const line = lines[index].trimEnd();
    const trimmed = line.trim();
    const leading = line.length - line.trimStart().length;
    const indent = line.slice(0, leading).replace(/\t/g, "    ").length;

    if (!trimmed) {
      if (!LIST_ITEM.test(lines[index + 1] ?? "")) closeLists();
      continue;
    }

    const fence = /^(`{3,}|~{3,})(.*)$/.exec(trimmed);
    if (fence) {
      closeLists(indent - 1);
      const code: string[] = [];
      const closeFence = new RegExp(`^${fence[1][0]}{${fence[1].length},}$`);
      while (index + 1 < lines.length && !closeFence.test(lines[index + 1].trim())) {
        const line = lines[++index];
        const remove = Math.min(leading, line.length - line.trimStart().length);
        code.push(line.slice(remove));
      }
      if (index + 1 < lines.length) index++;
      html.push(`<pre><code>${escapeHtml(code.join("\n"))}</code></pre>`);
      continue;
    }

    if (trimmed.startsWith(">") && quoteDepth < 32) {
      closeLists(indent - 1);
      const quote = [trimmed.replace(/^> ?/, "")];
      while (lines[index + 1]?.trimStart().startsWith(">")) {
        quote.push(lines[++index].trimStart().replace(/^> ?/, ""));
      }
      html.push("<blockquote>", renderBlocks(quote, quoteDepth + 1), "</blockquote>");
      continue;
    }

    if (trimmed.startsWith("|")) {
      const header = splitTableRow(trimmed);
      if (isTableDelimiter(lines[index + 1], header.length)) {
        closeLists(indent - 1);
        index++;
        const table = [`<table><tbody><tr>${header.map(cell => `<th>${inlineMarkdownToHtml(cell)}</th>`).join("")}</tr>`];
        while (lines[index + 1]?.trim().startsWith("|")) {
          const row = splitTableRow(lines[++index]);
          table.push(`<tr>${header.map((_, column) => `<td>${inlineMarkdownToHtml(row[column] ?? "")}</td>`).join("")}</tr>`);
        }
        table.push("</tbody></table>");
        html.push(table.join(""));
        continue;
      }
    }

    const heading = /^(#{1,6})\s+(.+)$/.exec(trimmed);
    if (heading) {
      closeLists(indent - 1);
      const level = Math.min(heading[1].length, 6);
      html.push(`<h${level}>${inlineMarkdownToHtml(heading[2])}</h${level}>`);
      continue;
    }

    if (/^[-*_]{3,}$/.test(trimmed)) {
      closeLists(indent - 1);
      html.push("<hr />");
      continue;
    }

    const item = LIST_ITEM.exec(line);
    if (item) {
      const [, , marker, checked, content] = item;
      const prefix = checked === undefined ? "" : checked === " " ? "☐ " : "☑ ";
      const ordered = /^\d/.test(marker);
      addItem(indent, ordered ? "ol" : "ul", prefix + inlineMarkdownToHtml(content),
        ordered ? Number.parseInt(marker, 10) : undefined);
      continue;
    }

    closeLists(indent - 1);
    html.push(`<p>${inlineMarkdownToHtml(trimmed)}</p>`);
  }

  closeLists();
  return html.join("\n");
}

export function markdownToConfluenceHtml(markdown: string, headerHtml = ""): string {
  const lines = markdown.replace(/\r\n?/g, "\n").split("\n");
  return `<div data-clawscribe-confluence-draft="true">${headerHtml}${renderBlocks(lines)}</div>`;
}

export async function writeConfluenceDraftToClipboard(
  { markdown, html }: MeetingExportHeader,
): Promise<ClipboardWriteMode> {
  const ClipboardItemCtor = window.ClipboardItem;

  if (navigator.clipboard?.write && ClipboardItemCtor) {
    const item = new ClipboardItemCtor({
      "text/html": new Blob([html], { type: "text/html" }),
      "text/plain": new Blob([markdown], { type: "text/plain" }),
    });
    try {
      await navigator.clipboard.write([item]);
      return "rich";
    } catch {
      // Some editors or WebViews accept only the text clipboard format.
    }
  }

  await navigator.clipboard.writeText(markdown);
  return "markdown";
}
