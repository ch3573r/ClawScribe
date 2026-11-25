import { findRanges } from './transcript-find';

/** Locate each match within its formatted text spans, including across links. */
export function findSummaryText(spans: readonly string[], query: string) {
  let span = 0;
  let spanStart = 0;
  function point(position: number, isEnd: boolean) {
    while (span < spans.length - 1) {
      const spanEnd = spanStart + spans[span].length;
      if (isEnd ? position <= spanEnd : position < spanEnd) break;
      spanStart = spanEnd;
      span++;
    }
    return { span, offset: position - spanStart };
  }
  return findRanges(spans.join(''), query).map(({ start, end }) => ({
    start: point(start, false), end: point(end, true),
  }));
}

const TEXT_BLOCK = 'p, h1, h2, h3, h4, h5, h6, li, td, th, pre, [data-content-type]';

/** Build highlights without mutating the editor DOM or moving keyboard focus. */
export function collectSummaryRanges(root: HTMLElement, query: string): Range[] {
  const document = root.ownerDocument;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const ranges: Range[] = [];
  let block: Element | null = null;
  let spans: Text[] = [];
  const collectBlock = () => {
    for (const { start, end } of findSummaryText(spans.map(span => span.data), query)) {
      const range = document.createRange();
      range.setStart(spans[start.span], start.offset);
      range.setEnd(spans[end.span], end.offset);
      ranges.push(range);
    }
    spans = [];
  };
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const text = node as Text;
    if (!text.data) continue;
    const textBlock = text.parentElement?.closest(TEXT_BLOCK) ?? root;
    if (textBlock !== block) {
      collectBlock();
      block = textBlock;
    }
    spans.push(text);
  }
  collectBlock();
  return ranges;
}
