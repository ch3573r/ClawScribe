import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, deferred, flush } from './hook-harness.mjs';

const jsx = (type, props) => ({ type, props });
const nodes = node => Array.isArray(node) ? node.flatMap(nodes)
  : node && typeof node === 'object' ? [node, ...nodes(node.props?.children)] : [];
const text = node => typeof node === 'string' || typeof node === 'number' ? String(node)
  : Array.isArray(node) ? node.map(text).join('') : node ? text(node.props?.children) : '';

test('saved answers render cited Markdown, append with provenance, and recheck each jump', async () => {
  const hooks = createHookHarness();
  const note = {
    id: 'note', content: '**Decision** [K1]', provider: 'openai', model: 'fixture',
    context: { session_id: 'session', finalized_through_seconds: 60, transcription_incomplete: false },
    cited_tags: [1], question_tags: [],
  };
  const target = { meeting_id: 'meeting', transcript_id: 'row', transcript_index: 3, start_seconds: 42 };
  const citation = { text: 'A decision', start_seconds: 42, historical: true, navigation: target };
  const revealed = [], played = [], appended = [];
  let navigations = 0;
  let revealGuard;
  let revealPending;
  let reads = 0;
  let citationAvailable = true;
  const { SavedLiveAnswers } = loadTsModule('src/components/MeetingDetails/SavedLiveAnswers.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    'next/navigation': { useRouter: () => ({ push: () => assert.fail('expected an in-meeting citation jump') }) },
    sonner: { toast: { success() {}, error() {} } },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      if (command === 'list_saved_live_answers') return [note];
      assert.equal(command, 'resolve_saved_live_answer_citation');
      assert.equal(args.answerId, note.id);
      assert.equal(args.tag, 1);
      reads++;
      return citationAvailable ? citation : null;
    } },
    '@/components/Knowledge/KnowledgeChat': { CitedMarkdown: 'markdown' },
    '@/components/ui/button': { Button: 'button' },
    '@/components/ui/dialog': { Dialog: 'dialog', DialogContent: 'content', DialogHeader: 'header',
      DialogDescription: 'description', DialogTitle: 'title', DialogFooter: 'footer' },
    '@/components/ui/tooltip': { Tooltip: 'tooltip', TooltipContent: 'tip', TooltipTrigger: 'trigger' },
  });
  const render = () => hooks.render(() => SavedLiveAnswers({
    meetingId: 'meeting', summaryBusy: false,
    onPlay: async seconds => { played.push(seconds); },
    onReveal: async (row, current) => {
      revealed.push(row.transcript_id);
      revealGuard = current;
      await revealPending?.promise;
    },
    onNavigate: () => { navigations++; },
    onInsert: async markdown => { appended.push(markdown); },
  }));
  const button = title => nodes(render()).find(node => node.type === 'button' && text(node) === title);
  async function inspect() {
    const markdown = nodes(render()).find(node => node.type === 'markdown');
    assert.equal(markdown.props.content, note.content);
    markdown.props.renderCitation(1, 'source').props.onClick();
    await flush();
  }
  try {
    render();
    await flush();
    button('Saved live answers (1)').props.onClick();
    assert.ok(button('Copy'));
    await inspect();
    button('Show in transcript').props.onClick();
    await flush();
    assert.deepEqual(revealed, ['row']);
    assert.equal(revealGuard(), true);
    assert.equal(navigations, 0);
    assert.equal(nodes(render()).find(node => node.type === 'dialog').props.open, false);
    nodes(render()).find(node => node.type === 'content').props.onCloseAutoFocus({ preventDefault() {} });
    assert.equal(navigations, 1);
    assert.equal(reads, 2);
    button('Saved live answers (1)').props.onClick();
    await inspect();
    citationAvailable = false;
    button('Play from here').props.onClick();
    await flush();
    assert.deepEqual(played, []);
    assert.match(text(render()), /reference is no longer available/);
    citationAvailable = true;
    await inspect();
    button('Play from here').props.onClick();
    await flush();
    assert.deepEqual(played, [42]);
    button('Saved live answers (1)').props.onClick();
    button('Insert into summary').props.onClick();
    await flush();
    assert.match(appended[0], /\*\*Decision\*\* \[K1\]/);
    assert.match(appended[0], /Source numbers refer to this entry in Saved live answers/);
    button('Saved live answers (1)').props.onClick();
    await inspect();
    revealPending = deferred();
    button('Show in transcript').props.onClick();
    await flush();
    assert.equal(revealGuard(), true);
    hooks.unmount();
    assert.equal(revealGuard(), false);
    revealPending.resolve();
    await flush();
    assert.equal(navigations, 1);
  } finally { hooks.unmount(); }
});
