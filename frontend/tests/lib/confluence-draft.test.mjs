import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

test('Confluence draft removes app citation targets and retains readable times', () => {
  const { buildConfluenceDraftMarkdown, markdownToConfluenceHtml } = loadTsModule('src/lib/confluenceDraft.ts');
  const summaryMarkdown = '- Send draft [00:12:34](#clawscribe-source-abc123)';
  const draft = buildConfluenceDraftMarkdown({ meetingId: 'meeting', meetingTitle: 'Notes', summaryMarkdown });
  assert.ok(draft.includes('(00:12:34)'));
  assert.ok(!draft.includes('clawscribe-source'));
  assert.ok(!markdownToConfluenceHtml(draft).includes('clawscribe-source'));
  assert.ok(summaryMarkdown.includes('clawscribe-source'));
});
