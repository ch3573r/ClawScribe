export interface ReplaceOptions { query: string; replacement: string; matchCase: boolean }
export interface ReplacePreview { token: string; matches: number; segments: number; examples: { before: string; after: string }[] }

// Work only on inline text. Never rewrite block IDs, link destinations or props.
// A match may cross formatting spans; inserted text inherits its first span's style.
export function planSummaryReplacement<T>(blocks: T, options: ReplaceOptions): ReplacePreview & { blocks: T } {
  if (!options.query || options.query.length > 1000 || options.replacement.length > 4000) {
    throw new Error('Enter search text (up to 1,000 characters) and a replacement up to 4,000 characters.');
  }
  const copy: T = globalThis.structuredClone(blocks);
  const pattern = new RegExp(options.query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), options.matchCase ? 'gu' : 'giu');
  let matches = 0;
  let segments = 0;
  const examples: ReplacePreview['examples'] = [];
  function inline(content: unknown[]) {
    const leaves: { text: string }[] = [];
    function collect(items: unknown[]) {
      for (const item of items) {
        if (!item || typeof item !== 'object') continue;
        const node = item as Record<string, unknown>;
        if (node.type === 'text' && typeof node.text === 'string') leaves.push(node as { text: string });
        else if (node.type === 'link' && Array.isArray(node.content)) collect(node.content);
      }
    }
    collect(content);
    const before = leaves.map(leaf => leaf.text).join('');
    const found = [...before.matchAll(pattern)];
    const after = before.replace(pattern, () => options.replacement);
    if (!found.length || before === after) return;
    const starts: number[] = [];
    leaves.reduce((offset, leaf) => { starts.push(offset); return offset + leaf.text.length; }, 0);
    // Reverse order keeps original offsets valid, including multiple matches in one span.
    for (const match of found.reverse()) {
      const start = match.index!;
      const end = start + match[0].length;
      for (let index = leaves.length - 1; index >= 0; index--) {
        const leaf = leaves[index];
        const leafStart = starts[index];
        const leafEnd = leafStart + leaf.text.length;
        if (leafStart >= end || leafEnd <= start) continue;
        leaf.text = leaf.text.slice(0, Math.max(0, start - leafStart))
          + (start >= leafStart ? options.replacement : '')
          + leaf.text.slice(Math.max(0, end - leafStart));
      }
    }
    matches += found.length;
    segments++;
    if (examples.length < 5) examples.push({ before, after });
  }
  function visit(value: unknown): void {
    if (Array.isArray(value)) { value.forEach(visit); return; }
    if (!value || typeof value !== 'object') return;
    const node = value as Record<string, unknown>;
    if (Array.isArray(node.content)) inline(node.content);
    else if (node.content && typeof node.content === 'object') visit(node.content);
    if (Array.isArray(node.children)) visit(node.children);
    if (Array.isArray(node.rows)) visit(node.rows);
    if (Array.isArray(node.cells)) {
      for (const cell of node.cells) Array.isArray(cell) ? inline(cell) : visit(cell);
    }
  }
  visit(copy);
  // The preview is local; compare the full snapshot so any intervening edit invalidates it.
  return { token: JSON.stringify([blocks, options]), matches, segments, examples, blocks: copy };
}
