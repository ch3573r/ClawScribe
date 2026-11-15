import { BlockNoteExtension } from '@blocknote/core';
import type { Mark, Node as ProseMirrorNode } from '@tiptap/pm/model';
import { Plugin, PluginKey } from '@tiptap/pm/state';
import { Decoration, DecorationSet } from '@tiptap/pm/view';

interface HeadingLabel { text: string; size: number; parts: number; bodyStartSize: number }
const sentencePunctuation = /[.!?…。！？؟۔՜՞։।॥።፧﹒﹖﹗｡．:：]/u;
const timestamp = /^(?:\d{1,2}:)?\d{1,2}:\d{2}$/;
const isBold = (node: ProseMirrorNode) => node.isText && node.marks.some(mark => mark.type.name === 'bold');

// A section label is structural, not language-specific. Positions remain in
// ProseMirror's UTF-16 units; the 60-character limit counts Unicode code points.
export function summaryHeadingLabel(node: ProseMirrorNode): HeadingLabel | null {
  if (node.type.name !== 'paragraph' || !node.firstChild || !isBold(node.firstChild)) return null;
  let text = '', size = 0, parts = 0;
  while (parts < node.childCount && isBold(node.child(parts))) {
    const child = node.child(parts++);
    text += child.text;
    size += child.nodeSize;
  }
  const next = parts < node.childCount ? node.child(parts) : null;
  if (next && !next.isText) return null;
  const label = text.trim().replace(/[:：]$/, '');
  if (!label.trim() || Array.from(text).length > 60 || sentencePunctuation.test(label)) return null;
  return { text, size, parts, bodyStartSize: next?.text ? Array.from(next.text)[0].length : 0 };
}

export function buildSummaryDecorations(doc: ProseMirrorNode): DecorationSet {
  const decorations: Decoration[] = [];
  doc.descendants((node, pos, parent) => {
    if (!node.inlineContent) return;
    const label = parent?.type.name === 'listItem' ? null : summaryHeadingLabel(node);
    if (label) {
      const from = pos + 1, to = from + label.size;
      decorations.push(Decoration.node(pos, pos + node.nodeSize, { class: 'meeting-summary-section' }));
      // Inline decorations can split across marks. Expose one semantic heading
      // only when the label is a single text fragment, avoiding fragment headings.
      decorations.push(Decoration.inline(from, to, {
        class: 'meeting-summary-label', 'data-summary-heading': 'true',
        ...(label.parts === 1 ? { role: 'heading', 'aria-level': '3' } : {}),
      }));
      if (label.bodyStartSize) decorations.push(Decoration.inline(to, to + label.bodyStartSize, { class: 'meeting-summary-body-start' }));
    }
    let run: { from: number; to: number; text: string; mark: Mark } | null = null;
    const flush = () => {
      if (run && timestamp.test(run.text.trim())) decorations.push(Decoration.inline(run.from, run.to, { 'data-summary-timestamp': 'true' }));
      run = null;
    };
    node.forEach((child, offset) => {
      const mark = child.isText ? child.marks.find(item => item.type.name === 'link') : undefined;
      const from = pos + 1 + offset;
      if (mark && run && mark.eq(run.mark) && run.to === from) {
        run.text += child.text; run.to += child.nodeSize;
      } else {
        flush();
        if (mark) run = { from, to: from + child.nodeSize, text: child.text!, mark };
      }
    });
    flush();
  });
  return DecorationSet.create(doc, decorations);
}

export const summaryPresentationKey = new PluginKey<DecorationSet>('meeting-summary-presentation');
export function createSummaryPresentationPlugin(): Plugin<DecorationSet> {
  return new Plugin<DecorationSet>({
    key: summaryPresentationKey,
    state: {
      init: (_, state) => buildSummaryDecorations(state.doc),
      apply: (transaction, previous, _oldState, state) => transaction.docChanged ? buildSummaryDecorations(state.doc) : previous,
    },
    props: { decorations: state => summaryPresentationKey.getState(state) },
  });
}
class SummaryPresentationExtension extends BlockNoteExtension {
  static key() { return 'meetingSummaryPresentation'; }
  constructor() { super(); this.addProsemirrorPlugin(createSummaryPresentationPlugin()); }
}
export function createSummaryPresentationExtension(): BlockNoteExtension { return new SummaryPresentationExtension(); }
