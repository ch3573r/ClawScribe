import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';
import { createHookHarness, flush } from './hook-harness.mjs';

test('action navigation keeps the meeting and action identities separate from route directives', () => {
  const { meetingActionUrl } = loadTsModule('src/lib/meeting-follow-through.ts');
  const url = new URL(meetingActionUrl('meeting&archiveAction=files', 'action#one'), 'https://example.com');
  assert.equal(url.pathname, '/meeting-details');
  assert.deepEqual(Object.fromEntries(url.searchParams), {
    id: 'meeting&archiveAction=files', from: 'archive', archiveAction: 'actions', actionId: 'action#one',
  });
});

test('series destination shortcuts require an explicit launch selection', () => {
  const { meetingActionUrl } = loadTsModule('src/lib/meeting-follow-through.ts');
  const url = new URL(meetingActionUrl('meeting', 'action'), 'https://example.com');
  assert.equal(url.searchParams.has('destination'), false);
});

test('bulk export groups selected open actions by meeting and preserves stable identities', () => {
  const { groupSelectedActions } = loadTsModule('src/lib/meeting-follow-through.ts');
  const rows = [
    { meetingId: 'a', meetingTitle: 'First', action: { id: 'same', done: false } },
    { meetingId: 'b', meetingTitle: 'Second', action: { id: 'same', done: false } },
    { meetingId: 'a', meetingTitle: 'First', action: { id: 'completed', done: true } },
    { meetingId: 'a', meetingTitle: 'First', action: { id: 'third', done: false } },
  ];
  const selected = new Set(rows.map(({ meetingId, action }) => JSON.stringify([meetingId, action.id])));
  assert.deepEqual(JSON.parse(JSON.stringify(groupSelectedActions(rows, selected))), [
    { meetingId: 'a', meetingTitle: 'First', actionIds: ['same', 'third'] },
    { meetingId: 'b', meetingTitle: 'Second', actionIds: ['same'] },
  ]);
});

test('the header count describes open actions and flags unreviewed suggestions', () => {
  const { meetingActionsLabel } = loadTsModule('src/lib/meeting-follow-through.ts');
  assert.equal(meetingActionsLabel(null), 'Actions');
  assert.equal(meetingActionsLabel({ openCount: 0, hasSuggestions: true }), 'Actions (0), new summary suggestions');
  assert.equal(meetingActionsLabel({ openCount: 3, hasSuggestions: false }), 'Actions (3)');
});

const jsx = (type, props) => ({ type, props });
const nodes = node => Array.isArray(node) ? node.flatMap(nodes) : node && typeof node === 'object'
  ? [node, ...nodes(node.props?.children)] : [];
const primitives = names => Object.fromEntries(names.split(' ').map(name => [name, name]));

test('Open actions saves inline changes and Undo reopens the matching stable action', async t => {
  const board = createHookHarness();
  const timers = [];
  const previous = globalThis.window;
  globalThis.window = { setTimeout: callback => timers.push(callback), clearTimeout() {} };
  t.after(() => { board.unmount(); globalThis.window = previous; });
  const calls = [];
  let undo;
  const action = { id: 'stable', title: 'Send quote', owner: 'Dana', dueDate: null, done: false };
  const { OpenMeetingActions } = loadTsModule('src/components/Archive/OpenMeetingActions.tsx', {
    react: board.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    'next/link': { __esModule: true, default: 'Link' },
    'next/navigation': { useRouter: () => ({ push() {} }) },
    'lucide-react': {}, sonner: { toast: { success: (_, options) => { undo = options.action; }, error() {}, dismiss() {} } },
    '@tauri-apps/api/event': { listen: async () => () => {} },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push([command, JSON.parse(JSON.stringify(args))]);
      return command === 'list_open_reviewed_actions'
        ? { rows: [{ action, meetingId: 'meeting', meetingTitle: 'Review', createdAt: '2026-11-25' }], series: [], hasMore: false }
        : [];
    } },
    '@/components/ui/button': primitives('Button'),
    '@/components/ui/checkbox': primitives('Checkbox'),
    '@/components/ui/input': primitives('Input'),
    '@/components/ui/label': primitives('Label'),
    '@/components/ui/page-section': primitives('PageSection'),
    '@/components/ui/dropdown-menu': primitives('DropdownMenu DropdownMenuContent DropdownMenuItem DropdownMenuTrigger'),
    '@/components/ui/select': primitives('Select SelectContent SelectItem SelectTrigger SelectValue'),
    '@/components/MeetingDetails/PlannerExportPreview': primitives('PlannerExportPreview'),
    '@/components/MeetingDetails/ToDoExportPreview': primitives('ToDoExportPreview'),
  });
  // The component and the row share the mocked React module. Render the row
  // after the parent's hooks in the harness, as a separate component slot.
  const render = () => board.render(() => {
    const tree = nodes(OpenMeetingActions());
    const row = tree.find(node => node.props?.row?.action.id === 'stable');
    return { tree, row: row ? nodes(row.type(row.props)) : [] };
  });
  const tick = async () => { timers.splice(0).forEach(callback => callback()); await flush(); };
  render();
  await tick();
  render().tree.find(node => node.props.id === 'follow-up-search').props.onChange({ target: { value: 'quote' } });
  render();
  await tick();
  assert.equal(calls.at(-1)[1].filters.search, 'quote');
  render().row.find(node => node.props['aria-label']?.startsWith('Edit owner')).props.onClick();
  const fields = render().row.filter(node => node.type === 'Input');
  fields[0].props.onChange({ target: { value: ' Pat ' } });
  fields[1].props.onChange({ target: { value: '2026-11-28' } });
  await render().row.find(node => node.type === 'form').props.onSubmit({ preventDefault() {} });
  assert.deepEqual(calls.at(-1), ['edit_reviewed_meeting_actions', {
    meetingId: 'meeting', edits: [{ id: 'stable', owner: { from: 'Dana', to: 'Pat' }, dueDate: { from: null, to: '2026-11-28' } }],
  }]);
  render();
  await tick();
  render().row.find(node => node.props['aria-label']?.startsWith('Done:')).props.onCheckedChange(true);
  await flush();
  assert.deepEqual(calls.at(-1)[1].edits, [{ id: 'stable', done: { from: false, to: true } }]);
  assert.equal(undo.label, 'Undo');
  await undo.onClick();
  assert.deepEqual(calls.at(-1)[1].edits, [{ id: 'stable', done: { from: true, to: false } }]);
});

function emailReview(close, { recipients = [{ name: 'Dana', email: 'dana@example.com' }, { name: 'Pat', email: 'pat@example.com' }], initialDraft } = {}) {
  const hooks = createHookHarness();
  const calls = [];
  const { FollowUpEmail } = loadTsModule('src/components/MeetingDetails/FollowUpEmail.tsx', {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push([command, { ...args }]);
      if (command === 'get_reviewed_follow_up_draft') return {
        subject: 'Follow-up', body: 'Review the plan',
        recipients,
      };
    } },
    '@/components/ui/button': primitives('Button'),
    '@/components/ui/checkbox': primitives('Checkbox'),
    '@/components/ui/input': primitives('Input'),
    '@/components/ui/label': primitives('Label'),
    '@/components/ui/textarea': primitives('Textarea'),
    '@/components/ui/dialog': primitives('Dialog DialogContent DialogDescription DialogFooter DialogHeader DialogTitle'),
  });
  const render = () => nodes(hooks.render(() => FollowUpEmail({ meetingId: 'selected-meeting', close, initialDraft })));
  return { hooks, calls, render, find: (type, id) => render().find(node => node.type === type && (!id || node.props.id === id)) };
}

test('canceling a reviewed draft makes no mail client request', async () => {
  let closed = false;
  const review = emailReview(() => { closed = true; });
  try {
    review.render();
    await flush();
    assert.ok(review.render().filter(node => node.type === 'Checkbox').every(node => node.props.checked === false));
    review.find('Dialog').props.onOpenChange(false);
    assert.equal(closed, true);
    assert.deepEqual(review.calls.map(([command]) => command), ['get_reviewed_follow_up_draft']);
  } finally { review.hooks.unmount(); }
});

test('an ad-hoc meeting opens the AI draft without recipients and accepts validated manual addresses', async t => {
  const review = emailReview(() => {}, { recipients: [], initialDraft: { subject: 'Sprint & planning', body: 'Agreed next steps.' } });
  t.after(() => review.hooks.unmount());
  review.render();
  await flush();
  const submit = async () => { review.find('form').props.onSubmit({ preventDefault() {} }); await flush(); };
  const opens = () => review.calls.filter(([command]) => command === 'open_reviewed_follow_up_draft').map(([, args]) => args);
  assert.equal(review.render().find(node => node.props.type === 'submit').props.disabled, false);
  await submit();
  assert.equal(JSON.stringify(opens()[0]), JSON.stringify({ meetingId: 'selected-meeting', recipients: [], manualRecipients: [], subject: 'Sprint & planning', body: 'Agreed next steps.' }));

  const enter = value => review.find('Input', 'follow-up-manual-recipients').props.onChange({ target: { value } });
  enter('someone@example.com?bcc=wrong@example.com');
  await submit();
  assert.equal(opens().length, 1, 'an invalid address never reaches the native handoff');
  assert.equal(review.find('Input', 'follow-up-manual-recipients').props['aria-invalid'], true);
  for (const invalid of ['one%two@example.com', 'a'.repeat(65) + '@example.com']) {
    enter(invalid);
    await submit();
    assert.equal(opens().length, 1);
    assert.equal(review.find('Input', 'follow-up-manual-recipients').props['aria-invalid'], true);
  }
  enter('Dana+meeting@example.com; pat@example.com, PAT@example.com');
  await submit();
  assert.equal(JSON.stringify(opens()[1].manualRecipients), JSON.stringify(['Dana+meeting@example.com', 'pat@example.com']));
});

test('the mail client receives only the checked attendee and the edited draft on submit', async () => {
  const review = emailReview(() => {});
  try {
    review.render();
    await flush();
    review.find('Checkbox', 'follow-up-recipient-0').props.onCheckedChange(true);
    review.find('Textarea', 'follow-up-body').props.onChange({ target: { value: 'Updated reviewed note.' } });
    assert.equal(review.calls.length, 1);
    review.find('form').props.onSubmit({ preventDefault() {} });
    await flush();
    assert.equal(review.calls[1][0], 'open_reviewed_follow_up_draft');
    assert.deepEqual({ ...review.calls[1][1], recipients: [...review.calls[1][1].recipients], manualRecipients: [...review.calls[1][1].manualRecipients] }, {
      meetingId: 'selected-meeting', recipients: ['dana@example.com'], manualRecipients: [], subject: 'Follow-up', body: 'Updated reviewed note.',
    });
  } finally { review.hooks.unmount(); }
});

function exportPreview(kind, { groups, summary = false } = {}) {
  const hooks = createHookHarness();
  const calls = [];
  const items = [
    { localId: 'first', title: 'Send the report', owner: 'Dana', dueDate: null, details: 'Draft notes' },
    { localId: 'second', title: 'Check the plan', owner: null, dueDate: null },
  ];
  if (summary) items[0].originalTitleHash = '1234567890abcdef';
  const filename = kind === 'planner' ? 'PlannerExportPreview' : 'ToDoExportPreview';
  const preview = async (meetingId) => {
    calls.push(['preview', meetingId]);
    return { items, candidates: [], suggestionCount: 0 };
  };
  const send = async (...args) => {
    calls.push(['export', ...args]);
    return { overall: 'succeeded', items: [] };
  };
  const component = loadTsModule(`src/components/MeetingDetails/${filename}.tsx`, {
    react: hooks.react, 'react/jsx-runtime': { jsx, jsxs: jsx },
    'lucide-react': {}, sonner: { toast: { info() {}, success() {}, error(...args) { calls.push(['error', ...args]); } } },
    '@/contexts/ConfigContext': { useConfig: () => ({ modelConfig: { provider: 'openai', model: 'synthetic' } }) },
    '@/lib/exportDestinations': { getExportDestinations: () => ({ plannerAiPolish: true }) },
    '@tauri-apps/api/core': { invoke: async (command, args) => {
      calls.push([command, JSON.parse(JSON.stringify(args))]);
      if (command === 'edit_reviewed_meeting_actions') return [{
        id: 'first', title: 'Send reviewed report', details: 'Edited notes.', owner: 'Dana', dueDate: null,
      }];
      if (command === 'save_previewed_summary_actions') return [{
        id: 'reviewed-id', title: 'Send reviewed report', details: 'Check the attachment.', owner: 'Dana', dueDate: null,
        legacyAction: { id: 'first', titleHash: '1234567890abcdef' },
      }];
    } },
    '@/services/microsoftExportService': {
      taskIdentityReviewIds: () => new Set(),
      microsoftExportService: {
        previewPlannerTasks: preview,
        previewToDoTasks: preview,
        exportSelectedPlannerTasks: send,
        exportSelectedToDoTasks: send,
        listBuckets: async () => [{ id: 'bucket', name: 'Bucket' }],
        polishPlannerTasks: async (provider, model, actions) => {
          calls.push(['polish', provider, model, JSON.parse(JSON.stringify(actions))]);
          return [{ title: 'Send reviewed report', details: 'Check the attachment.' }];
        },
      },
    },
    '@/components/ui/button': primitives('Button'),
    '@/components/ui/checkbox': primitives('Checkbox'),
    '@/components/ui/input': primitives('Input'),
    '@/components/ui/textarea': primitives('Textarea'),
    '@/components/ui/select': primitives('Select SelectContent SelectItem SelectTrigger SelectValue'),
    '@/components/ui/dialog': primitives('Dialog DialogContent DialogDescription DialogFooter DialogHeader DialogTitle'),
  })[filename];
  const getMarkdown = async () => 'Summary';
  const props = {
    open: true, onOpenChange() {}, onReport() {},
    ...(groups ? { actionGroups: groups } : { meetingId: 'meeting', meetingTitle: 'Meeting', getMarkdown }),
    ...(kind === 'planner' ? { planId: 'plan', defaultBucketId: 'bucket' } : { listId: 'list' }),
  };
  const render = () => nodes(hooks.render(() => {
    const node = component(props);
    return typeof node.type === 'function' ? node.type(node.props) : node;
  }));
  const click = label => render().find(node => node.type === 'Button' && node.props.children === label).props.onClick();
  return { hooks, calls, render, click };
}

for (const kind of ['planner', 'todo']) {
  test(`${kind} requires explicit polish and saves edited titles and notes locally without exporting`, async t => {
    const review = exportPreview(kind);
    t.after(() => review.hooks.unmount());
    review.render();
    await flush();
    assert.deepEqual(review.calls, [['preview', 'meeting']]);
    review.render().filter(node => node.type === 'Checkbox')[1].props.onCheckedChange(false);
    review.click('Polish selected with AI');
    await flush();
    assert.deepEqual(review.calls[1], ['polish', 'openai', 'synthetic', [{ title: 'Send the report', owner: 'Dana', dueDate: null }]]);
    assert.deepEqual(review.render().filter(node => node.type === 'Input').map(node => node.props.value), [
      'Send reviewed report', 'Check the plan',
    ]);
    review.render().find(node => node.type === 'Textarea').props.onChange({ target: { value: 'Edited notes.' } });
    assert.equal(review.calls.length, 2);
    review.click('Save edits');
    await flush();
    assert.deepEqual(review.calls[2], ['edit_reviewed_meeting_actions', {
      meetingId: 'meeting', edits: [{
        id: 'first', title: { from: 'Send the report', to: 'Send reviewed report' },
        details: { from: null, to: 'Edited notes.' },
      }],
    }]);
    assert.equal(review.calls.some(([command]) => command === 'export'), false);
  });
}

test('saving an edited summary draft reuses the same action when the user confirms export', async t => {
  const review = exportPreview('todo', { summary: true });
  t.after(() => review.hooks.unmount());
  review.render();
  await flush();
  review.render().filter(node => node.type === 'Checkbox')[1].props.onCheckedChange(false);
  review.click('Polish selected with AI');
  await flush();
  review.click('Save edits');
  await flush();
  assert.deepEqual(review.calls[2], ['save_previewed_summary_actions', {
    meetingId: 'meeting', markdown: 'Summary',
    edits: [{ localId: 'first', originalTitleHash: '1234567890abcdef', title: 'Send reviewed report', details: 'Check the attachment.' }],
  }]);
  assert.equal(review.calls.length, 3);
  review.click('Export 1 task');
  await flush();
  const [command, meetingId, , destination, tasks] = review.calls[3];
  assert.deepEqual([command, meetingId, destination], ['export', 'meeting', 'list']);
  assert.equal(tasks[0].localId, 'reviewed-id');
  assert.equal(tasks[0].details, 'Check the attachment.');
});

test('bulk export only submits the confirmed rows under their own meeting IDs', async t => {
  const groups = [
    { meetingId: 'a', meetingTitle: 'First', actionIds: ['first'] },
    { meetingId: 'b', meetingTitle: 'Second', actionIds: ['first'] },
    { meetingId: 'c', meetingTitle: 'Third', actionIds: ['second'] },
  ];
  const review = exportPreview('planner', { groups });
  t.after(() => review.hooks.unmount());
  review.render();
  await flush();
  assert.deepEqual(review.calls, [['preview', 'a'], ['preview', 'b'], ['preview', 'c']]);
  review.render().filter(node => node.type === 'Checkbox')[1].props.onCheckedChange(false);
  review.click('Export 2 tasks');
  await flush();
  assert.deepEqual(review.calls.slice(3).map(([command, meetingId, , destination, tasks]) => [
    command, meetingId, destination, ...tasks.map(task => [task.localId, task.bucketId]),
  ]), [['export', 'a', 'plan', ['first', 'bucket']], ['export', 'c', 'plan', ['second', 'bucket']]]);
});
