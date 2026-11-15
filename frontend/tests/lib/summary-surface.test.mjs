import assert from 'node:assert/strict';
import test from 'node:test';
import { createRequire } from 'node:module';
import { loadTsModule } from './load-ts-module.mjs';
const require = createRequire(import.meta.url);
const { Schema } = require('@tiptap/pm/model');
const { EditorState, TextSelection } = require('@tiptap/pm/state');
const schema = new Schema({
  nodes: {
    doc: { content: 'block+' }, paragraph: { group: 'block', content: 'inline*' },
    bulletListItem: { group: 'block', content: 'inline*' }, listItem: { group: 'block', content: 'paragraph' },
    text: { group: 'inline' }, hardBreak: { group: 'inline', inline: true },
  },
  marks: { bold: {}, italic: {}, link: { attrs: { href: {} } } },
});
const bold = text => schema.text(text, [schema.marks.bold.create()]);
const plain = text => schema.text(text);
const paragraph = content => schema.node('paragraph', null, content);
const helpers = () => loadTsModule('src/components/AISummary/summary-surface.ts');

for (const [name, node, expected] of [
  ['heading', paragraph([bold('Summary'), plain(' The team reviewed the plan.')]), 'Summary'],
  ['inline bold', paragraph([plain('Body first '), bold('Summary'), plain(' remains inline.')]), null],
  ['leading plain whitespace', paragraph([plain(' '), bold('Summary')]), null],
  ['list lead-in', schema.node('bulletListItem', null, [bold('Decision'), plain(' Retain it inline.')]), null],
  ['long bold sentence', paragraph([bold('This is a much longer bold statement that exceeds sixty characters by itself'), plain(' Body.')]), null],
  ['sentence punctuation', paragraph([bold('A sentence.'), plain(' Body.')]), null],
  ['label with colon', paragraph([bold('Key decisions:'), plain(' None noted.')]), 'Key decisions:'],
  ['middle colon', paragraph([bold('Key: decisions'), plain(' None noted.')]), null],
  ['double colon', paragraph([bold('Key decisions::'), plain(' None noted.')]), null],
  ['non-Latin label', paragraph([bold('主な決定事項'), plain(' 計画を継続します。')]), '主な決定事項'],
  ['standalone label', paragraph([bold('Puntos clave')]), 'Puntos clave'],
  ['non-Latin sentence punctuation', paragraph([bold('会議の結果。')]), null],
  ['bold label followed by a hard break', paragraph([bold('Summary'), schema.node('hardBreak'), plain('Body')]), null],
]) test('section detection: ' + name, () => {
  const range = helpers().summaryHeadingLabel(node);
  assert.equal(range?.text ?? null, expected);
});

test('a continuous bold run can span formatting marks without splitting its visual heading', () => {
  const node = paragraph([bold('Key '), schema.text('decisions', [schema.marks.bold.create(), schema.marks.italic.create()]), plain(' Body.')]);
  const range = helpers().summaryHeadingLabel(node);
  assert.equal(range.text, 'Key decisions');
  assert.equal(range.size, 'Key decisions'.length);
  assert.equal(range.parts, 2);
});

test('decorations leave the document, selection and composition metadata unchanged', () => {
  const { createSummaryPresentationPlugin, summaryPresentationKey } = helpers();
  const doc = schema.node('doc', null, [paragraph([bold('Summary'), plain(' Body '), schema.text('00:06', [schema.marks.link.create({ href: '#time' })])])]);
  const initial = JSON.stringify(doc.toJSON());
  let state = EditorState.create({ schema, doc, plugins: [createSummaryPresentationPlugin()] });
  assert.equal(JSON.stringify(state.doc.toJSON()), initial);
  const decorations = summaryPresentationKey.getState(state);
  const all = decorations.find();
  assert.ok(all.some(item => item.type.attrs?.['data-summary-heading'] === 'true'));
  assert.ok(all.some(item => item.type.attrs?.role === 'heading'));
  assert.ok(all.some(item => item.type.attrs?.['data-summary-timestamp'] === 'true'));
  const selection = TextSelection.create(state.doc, 2);
  state = state.apply(state.tr.setSelection(selection).setMeta('composition', 7));
  assert.equal(summaryPresentationKey.getState(state), decorations, 'selection/IME metadata must reuse the DecorationSet');
  assert.equal(state.selection.from, 2);
  assert.equal(JSON.stringify(state.doc.toJSON()), initial);
  const tr = state.tr.insertText('x').setMeta('composition', 8);
  const expected = JSON.stringify(tr.doc.toJSON());
  state = state.apply(tr);
  assert.notEqual(summaryPresentationKey.getState(state), decorations);
  assert.equal(JSON.stringify(state.doc.toJSON()), expected);
  assert.equal(tr.getMeta('composition'), 8);
});

test('generic list paragraphs keep lead-ins inline and non-timestamp links stay unchanged', () => {
  const { buildSummaryDecorations } = helpers();
  const doc = schema.node('doc', null, [schema.node('listItem', null, [paragraph([bold('Decision'), plain(' Body')])]), paragraph([schema.text('Website', [schema.marks.link.create({ href: 'https://example.com' })])])]);
  assert.equal(buildSummaryDecorations(doc).find().length, 0);
});

test('BlockNote extension registers exactly one presentation plugin', () => {
  const extension = helpers().createSummaryPresentationExtension();
  assert.equal(extension.constructor.key(), 'meetingSummaryPresentation');
  assert.equal(extension.plugins.length, 1);
  assert.equal(extension.plugins[0].spec.appendTransaction, undefined);
  assert.equal(extension.plugins[0].spec.filterTransaction, undefined);
});
