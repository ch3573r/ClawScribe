import { searchHighlightParts, type SearchExcerpt } from "@/lib/archive-search";

export function SearchSnippet({ text, highlights, clipped_start, clipped_end }: SearchExcerpt) {
  return <>{clipped_start && '…'}{searchHighlightParts(text, highlights).map((part, index) => part.match
    ? <mark key={index} className="rounded-sm bg-accent text-accent-foreground">{part.text}</mark>
    : part.text)}{clipped_end && '…'}</>;
}
