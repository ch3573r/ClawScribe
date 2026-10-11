import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import { createElement, Fragment } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import ts from 'typescript';
import { loadTsModule } from './load-ts-module.mjs';

test('two transcript sharing controls have distinct IDs and matching labels', () => {
  const { TranscriptSharingControl } = loadTsModule('src/components/LiveTranscriptSharing.tsx', {
    '@/components/ui/switch': {
      Switch: ({ id, checked, disabled }) => createElement('button', {
        id, role: 'switch', 'aria-checked': checked, disabled,
      }),
    },
    '@/components/ui/page-section': { PageSection: 'section' },
    '@/hooks/useLiveAssistance': { useLiveAssistance: () => {} },
    '@/lib/knowledge-documents': { knowledgeProviderLabel: () => 'the selected provider' },
  });
  const live = {
    state: { provider: 'openai', transcriptSharing: false, sharingReady: true, sharingBusy: false },
    actions: { setTranscriptSharing: () => {} },
  };
  const markup = renderToStaticMarkup(createElement(Fragment, null,
    createElement(TranscriptSharingControl, live), createElement(TranscriptSharingControl, live)));
  const ids = [...markup.matchAll(/<button[^>]*\bid="([^"]+)"/g)].map(match => match[1]);
  const labels = [...markup.matchAll(/<label[^>]*\bfor="([^"]+)"/g)].map(match => match[1]);
  assert.equal(ids.length, 2);
  assert.equal(new Set(ids).size, 2, 'each component instance must own its switch ID');
  assert.deepEqual(labels, ids, 'each label must target its own switch');
});

test('both sharing call sites render JSX rather than calling the component as a function', () => {
  for (const path of ['src/components/LiveAssistancePanel.tsx', 'src/components/LiveTranscriptSharing.tsx']) {
    const source = fs.readFileSync(new URL(`../../${path}`, import.meta.url), 'utf8');
    const tree = ts.createSourceFile(path, source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
    let instances = 0;
    function inspect(node) {
      if (ts.isCallExpression(node)) {
        assert.notEqual(node.expression.getText(tree), 'TranscriptSharingControl', path);
      }
      if ((ts.isJsxSelfClosingElement(node) || ts.isJsxOpeningElement(node))
          && node.tagName.getText(tree) === 'TranscriptSharingControl') instances++;
      ts.forEachChild(node, inspect);
    }
    inspect(tree);
    assert.equal(instances, 1, `${path} must render one component instance`);
  }
  const source = fs.readFileSync(new URL('../../src/components/LiveTranscriptSharing.tsx', import.meta.url), 'utf8');
  assert.match(source, /\buseId\(\)/, 'IDs must come from the React instance lifecycle');
});
